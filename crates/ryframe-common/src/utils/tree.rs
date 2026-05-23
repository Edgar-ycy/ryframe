#![allow(dead_code)]

use serde::Serialize;

/// 树节点 trait
///
/// 任何需要构建树结构的实体实现此 trait 即可。
pub trait TreeNode: Clone {
    type Id: PartialEq + Clone;

    /// 节点 ID
    fn id(&self) -> &Self::Id;
    /// 父节点 ID（None 表示根节点）
    fn parent_id(&self) -> Option<Self::Id>;
    /// 子节点列表的可变引用
    fn children_mut(&mut self) -> &mut Vec<Self>;
}

/// 将扁平列表构建为树（从 parent_id 为 None 的根节点开始）
///
/// items 会被消费，返回根节点列表。
pub fn build_tree<T: TreeNode>(items: &[T], parent_id: Option<T::Id>) -> Vec<T>
where
    T::Id: PartialEq,
{
    items
        .iter()
        .filter(|node| match (&parent_id, node.parent_id()) {
            (None, None) => true,
            (Some(pid), Some(node_pid)) => pid.eq(&node_pid),
            _ => false,
        })
        .map(|node| {
            let mut cloned = node.clone();
            let children = build_tree(items, Some(node.id().clone()));
            *cloned.children_mut() = children;
            cloned
        })
        .collect()
}

/// 树节点转 DTO 的通用输出结构
#[derive(Debug, Clone, Serialize)]
pub struct TreeNodeDto<T: Serialize> {
    #[serde(flatten)]
    pub data: T,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<TreeNodeDto<T>>,
}

impl<T: Serialize + Clone> TreeNodeDto<T> {
    pub fn new(data: T) -> Self {
        Self {
            data,
            children: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, Serialize)]
    struct TestNode {
        id: i32,
        parent_id: Option<i32>,
        name: String,
        children: Vec<TestNode>,
    }

    impl TreeNode for TestNode {
        type Id = i32;

        fn id(&self) -> &Self::Id {
            &self.id
        }

        fn parent_id(&self) -> Option<Self::Id> {
            self.parent_id
        }

        fn children_mut(&mut self) -> &mut Vec<Self> {
            &mut self.children
        }
    }

    fn make_node(id: i32, parent_id: Option<i32>, name: &str) -> TestNode {
        TestNode {
            id,
            parent_id,
            name: name.to_string(),
            children: vec![],
        }
    }

    #[test]
    fn test_build_tree() {
        // 空列表
        let empty: Vec<TestNode> = vec![];
        assert!(build_tree(&empty, None).is_empty());

        // 单根多子节点
        let nodes = vec![
            make_node(1, None, "root"),
            make_node(2, Some(1), "child1"),
            make_node(3, Some(1), "child2"),
            make_node(4, Some(2), "grandchild"),
        ];
        let roots = build_tree(&nodes, None);
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].children.len(), 2);
        let child1 = roots[0].children.iter().find(|n| n.id == 2).unwrap();
        assert_eq!(child1.children.len(), 1);

        // 多根
        let nodes2 = vec![
            make_node(1, None, "r1"),
            make_node(2, None, "r2"),
            make_node(3, Some(1), "c1"),
        ];
        assert_eq!(build_tree(&nodes2, None).len(), 2);

        // DTO 序列化
        #[derive(Debug, Clone, Serialize)]
        struct Inner {
            name: String,
        }
        let dto = TreeNodeDto::new(Inner { name: "x".into() });
        let json = serde_json::to_string(&dto).unwrap();
        assert!(json.contains("x"));
        assert!(!json.contains("children"));
    }
}
