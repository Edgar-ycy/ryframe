use ryframe_common::utils::tree::{TreeNode, TreeNodeDto, build_tree};
use serde::Serialize;

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
    let empty: Vec<TestNode> = vec![];
    assert!(build_tree(&empty, None).is_empty());

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

    let nodes2 = vec![
        make_node(1, None, "r1"),
        make_node(2, None, "r2"),
        make_node(3, Some(1), "c1"),
    ];
    assert_eq!(build_tree(&nodes2, None).len(), 2);

    #[derive(Debug, Clone, Serialize)]
    struct Inner {
        name: String,
    }
    let dto = TreeNodeDto::new(Inner { name: "x".into() });
    let json = serde_json::to_string(&dto).unwrap();
    assert!(json.contains("x"));
    assert!(!json.contains("children"));
}
