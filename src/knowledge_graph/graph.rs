use crate::db::schema::*;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use surrealdb::{RecordId, Surreal, engine::any::Any};

#[derive(Debug, Clone)]
pub struct KnowledgeGraph {
    pub nodes: HashMap<RecordId, GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub type_system: TypeSystem,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphNode {
    pub id: RecordId,
    pub node_type: NodeType,
    pub data: serde_json::Value,
    pub embeddings: Option<Vec<EmbeddingInfo>>,
    pub metadata: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
#[derive(PartialEq)]
pub enum NodeType {
    Service,
    Tool,
    Type,
    Concept,
    Registry,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingInfo {
    pub model: String,
    pub vector: Vec<f32>,
    pub content_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphEdge {
    pub id: RecordId,
    pub from: RecordId,
    pub to: RecordId,
    pub edge_type: EdgeType,
    pub weight: f32,
    pub metadata: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum EdgeType {
    /// Tool produces output that can be input to another tool
    DataFlow,
    /// Tools are semantically related
    SemanticSimilarity,
    /// Tools often used in sequence
    Sequential,
    /// Tools can be used in parallel
    Parallel,
    /// One tool conditionally follows another
    Conditional,
    /// One tool transforms data to match another's input
    Transform,
    /// Tool belongs to a service
    BelongsTo,
    /// Type relationship (is-a, has-a, etc.)
    TypeRelation,
    /// Concept relationship
    ConceptRelation,
}

#[derive(Debug, Clone)]
pub struct TypeSystem {
    types: HashMap<String, TypeInfo>,
    compatibility_rules: Vec<CompatibilityRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeInfo {
    pub name: String,
    pub base_type: Option<String>,
    pub properties: HashMap<String, TypeProperty>,
    pub enum_values: Option<Vec<serde_json::Value>>,
    pub is_array: bool,
    pub is_optional: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeProperty {
    pub property_type: String,
    pub required: bool,
    pub description: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CompatibilityRule {
    pub from_type: String,
    pub to_type: String,
    pub transformation: Option<String>,
    pub confidence: f32,
    pub description: String,
}

impl Default for KnowledgeGraph {
    fn default() -> Self {
        Self::new()
    }
}

impl KnowledgeGraph {
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            edges: Vec::new(),
            type_system: TypeSystem::new(),
        }
    }

    pub fn add_node(&mut self, node: GraphNode) {
        self.nodes.insert(node.id.clone(), node);
    }

    pub fn add_edge(&mut self, edge: GraphEdge) {
        // Check if nodes exist
        if self.nodes.contains_key(&edge.from) && self.nodes.contains_key(&edge.to) {
            self.edges.push(edge);
        }
    }

    pub fn get_node(&self, id: &RecordId) -> Option<&GraphNode> {
        self.nodes.get(id)
    }

    pub fn get_neighbors(
        &self,
        node_id: &RecordId,
        edge_type: Option<EdgeType>,
    ) -> Vec<&GraphEdge> {
        self.edges
            .iter()
            .filter(|edge| {
                let matches_node = &edge.from == node_id || &edge.to == node_id;
                let matches_type = if let Some(ref et) = edge_type {
                    edge.edge_type == *et
                } else {
                    true
                };
                matches_node && matches_type
            })
            .collect()
    }

    /// Returns neighbor nodes and the connecting edges for the given node,
    /// filtered by one or more edge types.
    pub fn neighbors_of(
        &self,
        node_id: &RecordId,
        types: &[EdgeType],
    ) -> Vec<(&RecordId, &GraphEdge)> {
        self.edges
            .iter()
            .filter(|edge| {
                (edge.from == *node_id || edge.to == *node_id) && types.contains(&edge.edge_type)
            })
            .map(|edge| {
                let other = if edge.from == *node_id {
                    &edge.to
                } else {
                    &edge.from
                };
                (other, edge)
            })
            .collect()
    }

    #[allow(clippy::mutable_key_type)]
    pub fn find_path(
        &self,
        from: &RecordId,
        to: &RecordId,
        max_depth: usize,
        allowed_edges: Option<Vec<EdgeType>>,
    ) -> Option<Vec<RecordId>> {
        let mut visited: HashSet<RecordId> = HashSet::new();
        let mut queue: VecDeque<(RecordId, Vec<RecordId>)> =
            VecDeque::from([(from.clone(), vec![from.clone()])]);
        let mut best_path: Option<Vec<RecordId>> = None;

        while let Some((current, path)) = queue.pop_front() {
            if visited.contains(&current) {
                continue;
            }
            visited.insert(current.clone());

            if &current == to {
                // If this is the first path or shorter than the best one, update best_path.
                if best_path
                    .as_ref()
                    .map(|p| path.len() < p.len())
                    .unwrap_or(true)
                {
                    best_path = Some(path.clone());
                }
                continue;
            }

            if path.len() >= max_depth {
                continue;
            }

            for edge in self.get_neighbors(&current, None) {
                // Respect allowed_edges if provided
                if let Some(ref allowed) = allowed_edges
                    && !allowed.contains(&edge.edge_type)
                {
                    continue;
                }

                let next = if edge.from == current {
                    edge.to.clone()
                } else {
                    edge.from.clone()
                };

                if !visited.contains(&next) {
                    let mut new_path = path.clone();
                    new_path.push(next.clone());
                    queue.push_back((next, new_path));
                }
            }
        }

        best_path
    }

    pub fn compute_similarity(&self, node1: &RecordId, node2: &RecordId) -> Option<f32> {
        // NOTE: We intentionally do not use `GraphNode.embeddings` here. Semantic
        // similarity is handled by the dedicated embedding layer (EmbeddingManager
        // + SurrealDB vector search). The graph focuses on structural/relational
        // similarity only (paths, edge types, etc.).

        // Path-based similarity (shorter path = more similar).
        // Distance is number of edges, which is path length - 1.
        if let Some(path) = self.find_path(node1, node2, 5, None)
            && path.len() > 1
        {
            let distance = (path.len() - 1) as f32;
            // Add 1.0 in the denominator to keep the score in (0, 1].
            return Some(1.0 / (1.0 + distance));
        }

        None
    }

    #[allow(clippy::mutable_key_type)]
    pub fn get_subgraph(&self, node_ids: &[RecordId]) -> KnowledgeGraph {
        let node_set: HashSet<&RecordId> = node_ids.iter().collect();
        let mut subgraph = KnowledgeGraph::new();

        // Add nodes
        for (id, node) in &self.nodes {
            if node_set.contains(id) {
                subgraph.nodes.insert(id.clone(), node.clone());
            }
        }

        // Add edges between nodes in the subgraph
        for edge in &self.edges {
            if node_set.contains(&edge.from) && node_set.contains(&edge.to) {
                subgraph.edges.push(edge.clone());
            }
        }

        subgraph.type_system = self.type_system.clone();
        subgraph
    }

    pub async fn build_from_database(db: &Surreal<Any>) -> Result<Self> {
        let mut graph = Self::new();

        // Load all services first so BelongsTo edges can be added safely.
        let services: Vec<ServiceRecord> = db.query("SELECT * FROM service").await?.take(0)?;

        for service in &services {
            let node = GraphNode {
                id: service.id.clone(),
                node_type: NodeType::Service,
                data: serde_json::to_value(service)?,
                embeddings: None,
                metadata: HashMap::new(),
            };
            graph.add_node(node);
        }

        // Load all tools and connect them to their services.
        let tools: Vec<ToolRecord> = db.query("SELECT * FROM tool").await?.take(0)?;

        for tool in tools {
            let node = GraphNode {
                id: tool.id.clone(),
                node_type: NodeType::Tool,
                data: serde_json::to_value(&tool)?,
                embeddings: None, // Will be loaded separately
                metadata: HashMap::new(),
            };
            graph.add_node(node);

            // Add a BelongsTo edge from the tool to its service.
            let edge_id = RecordId::from((
                "graph_edge",
                format!("belongs_to:{}:{}", tool.id, tool.service_id),
            ));

            let belongs_edge = GraphEdge {
                id: edge_id,
                from: tool.id.clone(),
                to: tool.service_id.clone(),
                edge_type: EdgeType::BelongsTo,
                weight: 1.0,
                metadata: HashMap::new(),
            };

            graph.add_edge(belongs_edge);
        }

        // Load compatibility edges
        let compatibilities: Vec<ToolCompatibility> = db
            .query("SELECT * FROM tool_compatibility")
            .await?
            .take(0)?;

        for compat in compatibilities {
            let edge = GraphEdge {
                id: compat.id,
                from: compat.r#in,
                to: compat.out,
                edge_type: match compat.compatibility_type {
                    CompatibilityType::DataFlow => EdgeType::DataFlow,
                    CompatibilityType::SemanticSimilarity => EdgeType::SemanticSimilarity,
                    CompatibilityType::Sequential => EdgeType::Sequential,
                    CompatibilityType::Parallel => EdgeType::Parallel,
                    CompatibilityType::Conditional => EdgeType::Conditional,
                    CompatibilityType::Transform => EdgeType::Transform,
                },
                weight: compat.confidence,
                metadata: HashMap::new(),
            };
            graph.add_edge(edge);
        }

        Ok(graph)
    }
}

impl Default for TypeSystem {
    fn default() -> Self {
        Self::new()
    }
}

impl TypeSystem {
    pub fn new() -> Self {
        Self {
            types: HashMap::new(),
            compatibility_rules: Vec::new(),
        }
    }

    pub fn add_type(&mut self, type_info: TypeInfo) {
        self.types.insert(type_info.name.clone(), type_info);
    }

    pub fn is_compatible(&self, from: &str, to: &str) -> Option<f32> {
        // Direct type match
        if from == to {
            return Some(1.0);
        }

        // Check compatibility rules
        for rule in &self.compatibility_rules {
            if rule.from_type == from && rule.to_type == to {
                return Some(rule.confidence);
            }
        }

        // Check inheritance (if from_type inherits from to_type)
        if let Some(base_type) = self.types.get(from).and_then(|ft| ft.base_type.as_ref()) {
            if base_type == to {
                return Some(0.8);
            }
            return self.is_compatible(base_type, to).map(|c| c * 0.8);
        }

        None
    }

    pub fn add_compatibility_rule(&mut self, rule: CompatibilityRule) {
        self.compatibility_rules.push(rule);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use surrealdb::RecordId;

    #[test]
    fn test_knowledge_graph_new() {
        let graph = KnowledgeGraph::new();
        assert!(graph.nodes.is_empty());
        assert!(graph.edges.is_empty());
        assert!(graph.type_system.types.is_empty());
        assert!(graph.type_system.compatibility_rules.is_empty());
    }

    #[test]
    fn test_add_node() {
        let mut graph = KnowledgeGraph::new();
        let node_id = RecordId::from(("tool", "test_tool"));

        let node = GraphNode {
            id: node_id.clone(),
            node_type: NodeType::Tool,
            data: json!({"name": "test_tool"}),
            embeddings: None,
            metadata: HashMap::new(),
        };

        graph.add_node(node);
        assert_eq!(graph.nodes.len(), 1);
        assert!(graph.nodes.contains_key(&node_id));
    }

    #[test]
    fn test_get_node() {
        let mut graph = KnowledgeGraph::new();
        let node_id = RecordId::from(("tool", "test_tool"));

        let node = GraphNode {
            id: node_id.clone(),
            node_type: NodeType::Tool,
            data: json!({"name": "test_tool"}),
            embeddings: None,
            metadata: HashMap::new(),
        };

        graph.add_node(node);

        let retrieved = graph.get_node(&node_id);
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().node_type, NodeType::Tool);

        let non_existent = graph.get_node(&RecordId::from(("tool", "nonexistent")));
        assert!(non_existent.is_none());
    }

    #[test]
    fn test_add_edge() {
        let mut graph = KnowledgeGraph::new();
        let node1_id = RecordId::from(("tool", "tool1"));
        let node2_id = RecordId::from(("tool", "tool2"));

        // Add nodes first
        graph.add_node(GraphNode {
            id: node1_id.clone(),
            node_type: NodeType::Tool,
            data: json!({"name": "tool1"}),
            embeddings: None,
            metadata: HashMap::new(),
        });

        graph.add_node(GraphNode {
            id: node2_id.clone(),
            node_type: NodeType::Tool,
            data: json!({"name": "tool2"}),
            embeddings: None,
            metadata: HashMap::new(),
        });

        let edge = GraphEdge {
            id: RecordId::from(("edge", "test_edge")),
            from: node1_id.clone(),
            to: node2_id.clone(),
            edge_type: EdgeType::DataFlow,
            weight: 0.9,
            metadata: HashMap::new(),
        };

        graph.add_edge(edge);
        assert_eq!(graph.edges.len(), 1);
    }

    #[test]
    fn test_add_edge_missing_nodes() {
        let mut graph = KnowledgeGraph::new();
        let node1_id = RecordId::from(("tool", "tool1"));
        let node2_id = RecordId::from(("tool", "tool2"));

        // Don't add nodes, just try to add edge
        let edge = GraphEdge {
            id: RecordId::from(("edge", "test_edge")),
            from: node1_id,
            to: node2_id,
            edge_type: EdgeType::DataFlow,
            weight: 0.9,
            metadata: HashMap::new(),
        };

        graph.add_edge(edge);
        assert_eq!(graph.edges.len(), 0); // Edge should not be added
    }

    #[test]
    fn test_get_neighbors() {
        let mut graph = KnowledgeGraph::new();
        let node1_id = RecordId::from(("tool", "tool1"));
        let node2_id = RecordId::from(("tool", "tool2"));
        let node3_id = RecordId::from(("tool", "tool3"));

        // Add nodes
        for (id, name) in [
            (node1_id.clone(), "tool1"),
            (node2_id.clone(), "tool2"),
            (node3_id.clone(), "tool3"),
        ] {
            graph.add_node(GraphNode {
                id,
                node_type: NodeType::Tool,
                data: json!({"name": name}),
                embeddings: None,
                metadata: HashMap::new(),
            });
        }

        // Add edges
        graph.add_edge(GraphEdge {
            id: RecordId::from(("edge", "edge1")),
            from: node1_id.clone(),
            to: node2_id.clone(),
            edge_type: EdgeType::DataFlow,
            weight: 0.9,
            metadata: HashMap::new(),
        });

        graph.add_edge(GraphEdge {
            id: RecordId::from(("edge", "edge2")),
            from: node1_id.clone(),
            to: node3_id.clone(),
            edge_type: EdgeType::Sequential,
            weight: 0.7,
            metadata: HashMap::new(),
        });

        // Get all neighbors
        let all_neighbors = graph.get_neighbors(&node1_id, None);
        assert_eq!(all_neighbors.len(), 2);

        // Get filtered neighbors
        let dataflow_neighbors = graph.get_neighbors(&node1_id, Some(EdgeType::DataFlow));
        assert_eq!(dataflow_neighbors.len(), 1);
        assert_eq!(dataflow_neighbors[0].edge_type, EdgeType::DataFlow);
    }

    #[test]
    fn test_neighbors_of() {
        let mut graph = KnowledgeGraph::new();
        let node1_id = RecordId::from(("tool", "tool1"));
        let node2_id = RecordId::from(("tool", "tool2"));
        let node3_id = RecordId::from(("tool", "tool3"));

        // Add nodes
        for (id, name) in [
            (node1_id.clone(), "tool1"),
            (node2_id.clone(), "tool2"),
            (node3_id.clone(), "tool3"),
        ] {
            graph.add_node(GraphNode {
                id,
                node_type: NodeType::Tool,
                data: json!({"name": name}),
                embeddings: None,
                metadata: HashMap::new(),
            });
        }

        // Add edges of different types
        graph.add_edge(GraphEdge {
            id: RecordId::from(("edge", "edge1")),
            from: node1_id.clone(),
            to: node2_id.clone(),
            edge_type: EdgeType::DataFlow,
            weight: 0.9,
            metadata: HashMap::new(),
        });

        graph.add_edge(GraphEdge {
            id: RecordId::from(("edge", "edge2")),
            from: node1_id.clone(),
            to: node3_id.clone(),
            edge_type: EdgeType::Sequential,
            weight: 0.7,
            metadata: HashMap::new(),
        });

        // Get neighbors of specific types
        let dataflow_neighbors = graph.neighbors_of(&node1_id, &[EdgeType::DataFlow]);
        assert_eq!(dataflow_neighbors.len(), 1);
        assert_eq!(*dataflow_neighbors[0].0, node2_id);

        let multiple_types =
            graph.neighbors_of(&node1_id, &[EdgeType::DataFlow, EdgeType::Sequential]);
        assert_eq!(multiple_types.len(), 2);

        let no_match = graph.neighbors_of(&node1_id, &[EdgeType::Parallel]);
        assert_eq!(no_match.len(), 0);
    }

    #[test]
    fn test_find_path_simple() {
        let mut graph = KnowledgeGraph::new();
        let node1_id = RecordId::from(("tool", "tool1"));
        let node2_id = RecordId::from(("tool", "tool2"));

        // Add nodes
        for (id, name) in [(node1_id.clone(), "tool1"), (node2_id.clone(), "tool2")] {
            graph.add_node(GraphNode {
                id,
                node_type: NodeType::Tool,
                data: json!({"name": name}),
                embeddings: None,
                metadata: HashMap::new(),
            });
        }

        // Add direct edge
        graph.add_edge(GraphEdge {
            id: RecordId::from(("edge", "edge1")),
            from: node1_id.clone(),
            to: node2_id.clone(),
            edge_type: EdgeType::DataFlow,
            weight: 0.9,
            metadata: HashMap::new(),
        });

        let path = graph.find_path(&node1_id, &node2_id, 5, None);
        assert!(path.is_some());
        assert_eq!(path.unwrap().len(), 2); // [from, to]
    }

    #[test]
    fn test_find_path_multi_hop() {
        let mut graph = KnowledgeGraph::new();
        let node1_id = RecordId::from(("tool", "tool1"));
        let node2_id = RecordId::from(("tool", "tool2"));
        let node3_id = RecordId::from(("tool", "tool3"));

        // Add nodes
        for (id, name) in [
            (node1_id.clone(), "tool1"),
            (node2_id.clone(), "tool2"),
            (node3_id.clone(), "tool3"),
        ] {
            graph.add_node(GraphNode {
                id,
                node_type: NodeType::Tool,
                data: json!({"name": name}),
                embeddings: None,
                metadata: HashMap::new(),
            });
        }

        // Add path: tool1 -> tool2 -> tool3
        graph.add_edge(GraphEdge {
            id: RecordId::from(("edge", "edge1")),
            from: node1_id.clone(),
            to: node2_id.clone(),
            edge_type: EdgeType::DataFlow,
            weight: 0.9,
            metadata: HashMap::new(),
        });

        graph.add_edge(GraphEdge {
            id: RecordId::from(("edge", "edge2")),
            from: node2_id.clone(),
            to: node3_id.clone(),
            edge_type: EdgeType::DataFlow,
            weight: 0.9,
            metadata: HashMap::new(),
        });

        let path = graph.find_path(&node1_id, &node3_id, 5, None);
        assert!(path.is_some());
        let path = path.unwrap();
        assert_eq!(path.len(), 3); // [tool1, tool2, tool3]
        assert_eq!(path[0], node1_id);
        assert_eq!(path[1], node2_id);
        assert_eq!(path[2], node3_id);
    }

    #[test]
    fn test_find_path_no_path() {
        let mut graph = KnowledgeGraph::new();
        let node1_id = RecordId::from(("tool", "tool1"));
        let node2_id = RecordId::from(("tool", "tool2"));

        // Add nodes but no edges
        for (id, name) in [(node1_id.clone(), "tool1"), (node2_id.clone(), "tool2")] {
            graph.add_node(GraphNode {
                id,
                node_type: NodeType::Tool,
                data: json!({"name": name}),
                embeddings: None,
                metadata: HashMap::new(),
            });
        }

        let path = graph.find_path(&node1_id, &node2_id, 5, None);
        assert!(path.is_none());
    }

    #[test]
    fn test_find_path_max_depth() {
        let mut graph = KnowledgeGraph::new();
        let node1_id = RecordId::from(("tool", "tool1"));
        let node2_id = RecordId::from(("tool", "tool2"));
        let node3_id = RecordId::from(("tool", "tool3"));

        // Add nodes
        for (id, name) in [
            (node1_id.clone(), "tool1"),
            (node2_id.clone(), "tool2"),
            (node3_id.clone(), "tool3"),
        ] {
            graph.add_node(GraphNode {
                id,
                node_type: NodeType::Tool,
                data: json!({"name": name}),
                embeddings: None,
                metadata: HashMap::new(),
            });
        }

        // Add path: tool1 -> tool2 -> tool3
        graph.add_edge(GraphEdge {
            id: RecordId::from(("edge", "edge1")),
            from: node1_id.clone(),
            to: node2_id.clone(),
            edge_type: EdgeType::DataFlow,
            weight: 0.9,
            metadata: HashMap::new(),
        });

        graph.add_edge(GraphEdge {
            id: RecordId::from(("edge", "edge2")),
            from: node2_id.clone(),
            to: node3_id.clone(),
            edge_type: EdgeType::DataFlow,
            weight: 0.9,
            metadata: HashMap::new(),
        });

        // Max depth too small
        let path = graph.find_path(&node1_id, &node3_id, 1, None);
        assert!(path.is_none());

        // Max depth sufficient
        let path = graph.find_path(&node1_id, &node3_id, 3, None);
        assert!(path.is_some());
    }

    #[test]
    fn test_find_path_with_allowed_edges() {
        let mut graph = KnowledgeGraph::new();
        let node1_id = RecordId::from(("tool", "tool1"));
        let node2_id = RecordId::from(("tool", "tool2"));

        // Add nodes
        for (id, name) in [(node1_id.clone(), "tool1"), (node2_id.clone(), "tool2")] {
            graph.add_node(GraphNode {
                id,
                node_type: NodeType::Tool,
                data: json!({"name": name}),
                embeddings: None,
                metadata: HashMap::new(),
            });
        }

        // Add edge of type DataFlow
        graph.add_edge(GraphEdge {
            id: RecordId::from(("edge", "edge1")),
            from: node1_id.clone(),
            to: node2_id.clone(),
            edge_type: EdgeType::DataFlow,
            weight: 0.9,
            metadata: HashMap::new(),
        });

        // Allow DataFlow edges
        let path = graph.find_path(&node1_id, &node2_id, 5, Some(vec![EdgeType::DataFlow]));
        assert!(path.is_some());

        // Only allow Sequential edges (should not find path)
        let path = graph.find_path(&node1_id, &node2_id, 5, Some(vec![EdgeType::Sequential]));
        assert!(path.is_none());
    }

    #[test]
    fn test_compute_similarity() {
        let mut graph = KnowledgeGraph::new();
        let node1_id = RecordId::from(("tool", "tool1"));
        let node2_id = RecordId::from(("tool", "tool2"));
        let node3_id = RecordId::from(("tool", "tool3"));

        // Add nodes
        for (id, name) in [
            (node1_id.clone(), "tool1"),
            (node2_id.clone(), "tool2"),
            (node3_id.clone(), "tool3"),
        ] {
            graph.add_node(GraphNode {
                id,
                node_type: NodeType::Tool,
                data: json!({"name": name}),
                embeddings: None,
                metadata: HashMap::new(),
            });
        }

        // Add edge: tool1 -> tool2
        graph.add_edge(GraphEdge {
            id: RecordId::from(("edge", "edge1")),
            from: node1_id.clone(),
            to: node2_id.clone(),
            edge_type: EdgeType::DataFlow,
            weight: 0.9,
            metadata: HashMap::new(),
        });

        // Direct connection should have similarity > 0
        let similarity = graph.compute_similarity(&node1_id, &node2_id);
        assert!(similarity.is_some());
        assert!(similarity.unwrap() > 0.0);

        // No connection should have no similarity
        let no_similarity = graph.compute_similarity(&node1_id, &node3_id);
        assert!(no_similarity.is_none());
    }

    #[test]
    fn test_get_subgraph() {
        let mut graph = KnowledgeGraph::new();
        let node1_id = RecordId::from(("tool", "tool1"));
        let node2_id = RecordId::from(("tool", "tool2"));
        let node3_id = RecordId::from(("tool", "tool3"));

        // Add nodes
        for (id, name) in [
            (node1_id.clone(), "tool1"),
            (node2_id.clone(), "tool2"),
            (node3_id.clone(), "tool3"),
        ] {
            graph.add_node(GraphNode {
                id,
                node_type: NodeType::Tool,
                data: json!({"name": name}),
                embeddings: None,
                metadata: HashMap::new(),
            });
        }

        // Add edges
        graph.add_edge(GraphEdge {
            id: RecordId::from(("edge", "edge1")),
            from: node1_id.clone(),
            to: node2_id.clone(),
            edge_type: EdgeType::DataFlow,
            weight: 0.9,
            metadata: HashMap::new(),
        });

        graph.add_edge(GraphEdge {
            id: RecordId::from(("edge", "edge2")),
            from: node2_id.clone(),
            to: node3_id.clone(),
            edge_type: EdgeType::Sequential,
            weight: 0.7,
            metadata: HashMap::new(),
        });

        // Get subgraph with only tool1 and tool2
        let subgraph = graph.get_subgraph(&[node1_id.clone(), node2_id.clone()]);

        assert_eq!(subgraph.nodes.len(), 2);
        assert!(subgraph.nodes.contains_key(&node1_id));
        assert!(subgraph.nodes.contains_key(&node2_id));
        assert!(!subgraph.nodes.contains_key(&node3_id));

        // Should only include edge between tool1 and tool2
        assert_eq!(subgraph.edges.len(), 1);
        assert_eq!(subgraph.edges[0].from, node1_id);
        assert_eq!(subgraph.edges[0].to, node2_id);
    }

    #[test]
    fn test_edge_type_serialization() {
        let edge_types = vec![
            EdgeType::DataFlow,
            EdgeType::SemanticSimilarity,
            EdgeType::Sequential,
            EdgeType::Parallel,
            EdgeType::Conditional,
            EdgeType::Transform,
            EdgeType::BelongsTo,
            EdgeType::TypeRelation,
            EdgeType::ConceptRelation,
        ];

        for edge_type in edge_types {
            let serialized = serde_json::to_string(&edge_type).unwrap();
            let deserialized: EdgeType = serde_json::from_str(&serialized).unwrap();
            assert_eq!(edge_type, deserialized);
        }
    }

    #[test]
    fn test_node_type_serialization() {
        let node_types = vec![
            NodeType::Service,
            NodeType::Tool,
            NodeType::Type,
            NodeType::Concept,
            NodeType::Registry,
        ];

        for node_type in node_types {
            let serialized = serde_json::to_string(&node_type).unwrap();
            let deserialized: NodeType = serde_json::from_str(&serialized).unwrap();
            assert_eq!(node_type, deserialized);
        }
    }

    #[test]
    fn test_type_system_new() {
        let type_system = TypeSystem::new();
        assert!(type_system.types.is_empty());
        assert!(type_system.compatibility_rules.is_empty());
    }

    #[test]
    fn test_type_system_add_type() {
        let mut type_system = TypeSystem::new();

        let type_info = TypeInfo {
            name: "string".to_string(),
            base_type: None,
            properties: HashMap::new(),
            enum_values: None,
            is_array: false,
            is_optional: false,
        };

        type_system.add_type(type_info);
        assert_eq!(type_system.types.len(), 1);
        assert!(type_system.types.contains_key("string"));
    }

    #[test]
    fn test_type_system_is_compatible_direct_match() {
        let mut type_system = TypeSystem::new();

        let type_info = TypeInfo {
            name: "string".to_string(),
            base_type: None,
            properties: HashMap::new(),
            enum_values: None,
            is_array: false,
            is_optional: false,
        };

        type_system.add_type(type_info);

        // Direct type match should return 1.0
        let compatibility = type_system.is_compatible("string", "string");
        assert_eq!(compatibility, Some(1.0));
    }

    #[test]
    fn test_type_system_is_compatible_no_match() {
        let type_system = TypeSystem::new();

        // No types defined, should return None
        let compatibility = type_system.is_compatible("string", "number");
        assert_eq!(compatibility, None);
    }

    #[test]
    fn test_type_system_compatibility_rules() {
        let mut type_system = TypeSystem::new();

        let rule = CompatibilityRule {
            from_type: "string".to_string(),
            to_type: "text".to_string(),
            transformation: None,
            confidence: 0.9,
            description: "String to text conversion".to_string(),
        };

        type_system.add_compatibility_rule(rule);

        let compatibility = type_system.is_compatible("string", "text");
        assert_eq!(compatibility, Some(0.9));
    }

    #[test]
    fn test_type_system_inheritance() {
        let mut type_system = TypeSystem::new();

        // Add base type
        let base_type = TypeInfo {
            name: "animal".to_string(),
            base_type: None,
            properties: HashMap::new(),
            enum_values: None,
            is_array: false,
            is_optional: false,
        };
        type_system.add_type(base_type);

        // Add derived type
        let derived_type = TypeInfo {
            name: "dog".to_string(),
            base_type: Some("animal".to_string()),
            properties: HashMap::new(),
            enum_values: None,
            is_array: false,
            is_optional: false,
        };
        type_system.add_type(derived_type);

        // Dog should be compatible with animal (inheritance)
        let compatibility = type_system.is_compatible("dog", "animal");
        assert_eq!(compatibility, Some(0.8));
    }

    #[test]
    fn test_graph_node_with_embeddings() {
        let node_id = RecordId::from(("tool", "test_tool"));
        let embedding = EmbeddingInfo {
            model: "test_model".to_string(),
            vector: vec![0.1, 0.2, 0.3],
            content_type: "text".to_string(),
        };

        let node = GraphNode {
            id: node_id.clone(),
            node_type: NodeType::Tool,
            data: json!({"name": "test_tool"}),
            embeddings: Some(vec![embedding]),
            metadata: HashMap::new(),
        };

        assert!(node.embeddings.is_some());
        let embeddings = node.embeddings.as_ref().unwrap();
        assert_eq!(embeddings.len(), 1);
        assert_eq!(embeddings[0].model, "test_model");
    }

    #[test]
    fn test_graph_node_serialization() {
        let node_id = RecordId::from(("tool", "test_tool"));
        let node = GraphNode {
            id: node_id.clone(),
            node_type: NodeType::Tool,
            data: json!({"name": "test_tool"}),
            embeddings: None,
            metadata: HashMap::new(),
        };

        let serialized = serde_json::to_string(&node).unwrap();
        let deserialized: GraphNode = serde_json::from_str(&serialized).unwrap();

        assert_eq!(node.id, deserialized.id);
        assert_eq!(node.node_type, deserialized.node_type);
        assert_eq!(node.data, deserialized.data);
    }

    #[test]
    fn test_graph_edge_serialization() {
        let edge = GraphEdge {
            id: RecordId::from(("edge", "test_edge")),
            from: RecordId::from(("tool", "tool1")),
            to: RecordId::from(("tool", "tool2")),
            edge_type: EdgeType::DataFlow,
            weight: 0.85,
            metadata: HashMap::new(),
        };

        let serialized = serde_json::to_string(&edge).unwrap();
        let deserialized: GraphEdge = serde_json::from_str(&serialized).unwrap();

        assert_eq!(edge.id, deserialized.id);
        assert_eq!(edge.from, deserialized.from);
        assert_eq!(edge.to, deserialized.to);
        assert_eq!(edge.edge_type, deserialized.edge_type);
        assert_eq!(edge.weight, deserialized.weight);
    }

    #[test]
    fn test_type_info_serialization() {
        let mut properties = HashMap::new();
        properties.insert(
            "length".to_string(),
            TypeProperty {
                property_type: "number".to_string(),
                required: true,
                description: Some("Length of the string".to_string()),
            },
        );

        let type_info = TypeInfo {
            name: "string".to_string(),
            base_type: Some("any".to_string()),
            properties,
            enum_values: Some(vec![json!("value1"), json!("value2")]),
            is_array: false,
            is_optional: true,
        };

        let serialized = serde_json::to_string(&type_info).unwrap();
        let deserialized: TypeInfo = serde_json::from_str(&serialized).unwrap();

        assert_eq!(type_info.name, deserialized.name);
        assert_eq!(type_info.base_type, deserialized.base_type);
        assert_eq!(type_info.properties.len(), deserialized.properties.len());
        assert_eq!(type_info.is_array, deserialized.is_array);
        assert_eq!(type_info.is_optional, deserialized.is_optional);
    }

    #[test]
    fn test_type_property_serialization() {
        let property = TypeProperty {
            property_type: "string".to_string(),
            required: false,
            description: Some("A string property".to_string()),
        };

        let serialized = serde_json::to_string(&property).unwrap();
        let deserialized: TypeProperty = serde_json::from_str(&serialized).unwrap();

        assert_eq!(property.property_type, deserialized.property_type);
        assert_eq!(property.required, deserialized.required);
        assert_eq!(property.description, deserialized.description);
    }

    #[test]
    fn test_embedding_info_serialization() {
        let embedding = EmbeddingInfo {
            model: "text-embedding-ada-002".to_string(),
            vector: vec![0.1, 0.2, 0.3, 0.4],
            content_type: "text".to_string(),
        };

        let serialized = serde_json::to_string(&embedding).unwrap();
        let deserialized: EmbeddingInfo = serde_json::from_str(&serialized).unwrap();

        assert_eq!(embedding.model, deserialized.model);
        assert_eq!(embedding.vector, deserialized.vector);
        assert_eq!(embedding.content_type, deserialized.content_type);
    }

    #[test]
    fn test_find_path_with_cycles() {
        let mut graph = KnowledgeGraph::new();
        let node1_id = RecordId::from(("tool", "tool1"));
        let node2_id = RecordId::from(("tool", "tool2"));
        let node3_id = RecordId::from(("tool", "tool3"));

        // Add nodes
        for (id, name) in [
            (node1_id.clone(), "tool1"),
            (node2_id.clone(), "tool2"),
            (node3_id.clone(), "tool3"),
        ] {
            graph.add_node(GraphNode {
                id,
                node_type: NodeType::Tool,
                data: json!({"name": name}),
                embeddings: None,
                metadata: HashMap::new(),
            });
        }

        // Create a longer path that will still be shorter than a cycle: tool1 -> tool2 -> tool3 -> tool4
        let node4_id = RecordId::from(("tool", "tool4"));
        graph.add_node(GraphNode {
            id: node4_id.clone(),
            node_type: NodeType::Tool,
            data: json!({"name": "tool4"}),
            embeddings: None,
            metadata: HashMap::new(),
        });

        graph.add_edge(GraphEdge {
            id: RecordId::from(("edge", "edge1")),
            from: node1_id.clone(),
            to: node2_id.clone(),
            edge_type: EdgeType::DataFlow,
            weight: 0.9,
            metadata: HashMap::new(),
        });

        graph.add_edge(GraphEdge {
            id: RecordId::from(("edge", "edge2")),
            from: node2_id.clone(),
            to: node3_id.clone(),
            edge_type: EdgeType::DataFlow,
            weight: 0.9,
            metadata: HashMap::new(),
        });

        graph.add_edge(GraphEdge {
            id: RecordId::from(("edge", "edge3")),
            from: node3_id.clone(),
            to: node4_id.clone(),
            edge_type: EdgeType::DataFlow,
            weight: 0.9,
            metadata: HashMap::new(),
        });

        // Add a cycle edge that creates a loop back but shouldn't create a shorter path
        graph.add_edge(GraphEdge {
            id: RecordId::from(("edge", "cycle_edge")),
            from: node4_id.clone(),
            to: node1_id.clone(),
            edge_type: EdgeType::DataFlow,
            weight: 0.9,
            metadata: HashMap::new(),
        });

        // Should still find direct path
        let path = graph.find_path(&node1_id, &node2_id, 5, None);
        assert!(path.is_some());
        assert_eq!(path.unwrap().len(), 2);

        // Should find path without infinite loop, taking the shortest route
        let path = graph.find_path(&node1_id, &node3_id, 5, None);
        assert!(path.is_some());
        assert_eq!(path.unwrap().len(), 3); // tool1 -> tool2 -> tool3
    }
}
