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
