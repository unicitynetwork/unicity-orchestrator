use crate::db::schema::*;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use surrealdb::{engine::any::Any, RecordId, Surreal};

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

    pub fn get_neighbors(&self, node_id: &RecordId, edge_type: Option<EdgeType>) -> Vec<&GraphEdge> {
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
                (edge.from == *node_id || edge.to == *node_id)
                    && types.contains(&edge.edge_type)
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
                if let Some(ref allowed) = allowed_edges {
                    if !allowed.iter().any(|et| edge.edge_type == *et) {
                        continue;
                    }
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
        if let Some(path) = self.find_path(node1, node2, 5, None) {
            if path.len() > 1 {
                let distance = (path.len() - 1) as f32;
                // Add 1.0 in the denominator to keep the score in (0, 1].
                return Some(1.0 / (1.0 + distance));
            }
        }

        None
    }

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
        let services: Vec<ServiceRecord> = db
            .query("SELECT * FROM service")
            .await?
            .take(0)?;

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
        let tools: Vec<ToolRecord> = db
            .query("SELECT * FROM tool")
            .await?
            .take(0)?;

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
        if let Some(from_type) = self.types.get(from) {
            if let Some(base_type) = &from_type.base_type {
                if base_type == to {
                    return Some(0.8);
                }
                return self.is_compatible(base_type, to).map(|c| c * 0.8);
            }
        }

        None
    }

    pub fn add_compatibility_rule(&mut self, rule: CompatibilityRule) {
        self.compatibility_rules.push(rule);
    }
}
