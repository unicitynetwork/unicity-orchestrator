use std::cmp::PartialEq;
use anyhow::{Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use rmcp::model::JsonObject;
use serde_json::Value;
use surrealdb::engine::any::Any;
use surrealdb::{RecordId, Surreal};

// TODO: Go full prolog style with unification, variable bindings, etc.
// For now we keep it simple with basic pattern matching on facts.
// This can be extended later.

/// A single symbolic rule used by the reasoning engine.
///
/// Rules consist of antecedent expressions (conditions) and consequent
/// expressions (facts or actions) that can be derived when the conditions
/// are satisfied. Confidence and priority can be used to rank or filter
/// rules when multiple apply.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolicRule {
    pub id: RecordId,
    pub name: String,
    pub description: String,
    pub antecedents: Vec<SymbolicExpression>,
    pub consequents: Vec<SymbolicExpression>,
    pub confidence: f32,
    pub priority: u32,
}

/// A symbolic expression used in rules, facts, and queries.
///
/// This forms a small logic language that can represent predicates,
/// logical connectives (AND/OR/NOT), implications, quantified expressions,
/// comparisons, variable references, and literal values.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SymbolicExpression {
    /// A fact or predicate
    Fact(Fact),
    /// Logical AND of expressions
    And(Vec<SymbolicExpression>),
    /// Logical OR of expressions
    Or(Vec<SymbolicExpression>),
    /// Logical NOT
    Not(Box<SymbolicExpression>),
    /// Logical implication (if-then)
    Implies(Box<SymbolicExpression>, Box<SymbolicExpression>),
    /// Quantified expression (for all, exists)
    Quantified(Quantifier, String, Box<SymbolicExpression>),
    /// Comparison
    Comparison(ComparisonOp, Box<SymbolicExpression>, Box<SymbolicExpression>),
    /// Variable reference
    Variable(String),
    /// Literal value
    Literal(LiteralValue),
}

/// A single fact in the working memory, represented as a predicate with arguments.
///
/// Facts can be asserted directly or derived by rules. Confidence can be
/// attached to indicate the reliability of the fact.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fact {
    pub predicate: String,
    pub arguments: Vec<SymbolicExpression>,
    pub confidence: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Quantifier {
    ForAll,
    Exists,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComparisonOp {
    Equals,
    NotEquals,
    GreaterThan,
    LessThan,
    GreaterEqual,
    LessEqual,
    Contains,
    StartsWith,
    EndsWith,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum LiteralValue {
    String(String),
    Number(f64),
    Boolean(bool),
    Array(Vec<LiteralValue>),
    Object(HashMap<String, LiteralValue>),
}

/// The mutable state over which the symbolic reasoner operates.
///
/// This holds asserted and derived facts, bound variables, and tool states.
/// It is passed into the rule engine for forward and backward chaining.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkingMemory {
    /// Holds all facts, grouped by predicate. Each predicate can have multiple facts.
    pub facts: HashMap<String, Vec<Fact>>,
    pub variables: HashMap<String, LiteralValue>,
    pub tool_states: HashMap<String, ToolState>,
}

/// Snapshot of the state of a single tool as seen by the symbolic layer.
///
/// This can be used to drive rules that reason about availability,
/// execution history, and expected inputs/outputs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolState {
    pub tool_id: RecordId,
    pub status: ToolStatus,
    pub last_output: Option<Value>,
    pub input_requirements: JsonObject,
    pub execution_count: u32,
    pub success_rate: f32,
}

/// High-level lifecycle status for tools as used in planning and inference.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Available,
    Executing,
    Completed,
    Failed,
    Blocked,
}

/// High-level entry point for symbolic reasoning over tools and user queries.
///
/// This type owns the rule set, working memory, and access to the database
/// for loading symbolic rules. It exposes convenience methods for:
/// - loading rules
/// - inferring tool selections for a query
/// - planning tool sequences to achieve a goal
pub struct SymbolicReasoner {
    db: Surreal<Any>,
    rules: Vec<SymbolicRule>,
    working_memory: WorkingMemory,
    rule_engine: RuleEngine,
}

impl SymbolicReasoner {
    pub fn new(db: Surreal<Any>) -> Self {
        Self {
            db,
            rules: Vec::new(),
            working_memory: WorkingMemory {
                facts: HashMap::new(),
                variables: HashMap::new(),
                tool_states: HashMap::new(),
            },
            rule_engine: RuleEngine::new(),
        }
    }

    /// Load all active symbolic rules from the database into memory.
    ///
    /// Rules are currently fetched from the `symbolic_rule` table and
    /// ordered by priority descending.
    pub async fn load_rules(&mut self) -> Result<()> {
        // Load symbolic rules from database
        let query = r#"
        SELECT * FROM symbolic_rule
        WHERE is_active = true
        ORDER BY priority DESC
        "#;

        let mut result = self.db.query(query).await?;
        let rules: Vec<SymbolicRule> = result.take(0)?;

        self.rules = rules;
        Ok(())
    }

    // TODO
    // /// Add a new symbolic rule to the in-memory rule set.
    // ///
    // /// NOTE: This currently only logs and appends to the in-memory list.
    // /// Persisting symbolic rules back into SurrealDB is left as a TODO.
    // pub async fn add_rule(&mut self, rule: SymbolicRule) -> Result<()> {
    //     let rule_id = rule.id.clone();
    //
    //     // Store in database (placeholder)
    //     tracing::info!("Adding rule: {}", rule_id);
    //
    //     self.rules.push(rule);
    //     Ok(())
    // }

    /// Use the symbolic engine to propose a set of tools for a natural language query.
    ///
    /// This parses the query into symbolic expressions, seeds the working memory
    /// with query and tool facts, runs forward chaining, and then extracts any
    /// `tool_selected(...)` facts produced by rules into concrete `ToolSelection`s.
    pub async fn infer_tool_selection(
        &mut self,
        query: &str,
        available_tools: &[crate::db::schema::ToolRecord],
        context: &HashMap<String, serde_json::Value>,
    ) -> Result<Vec<ToolSelection>> {
        // Parse query into symbolic representation
        let query_expr = self.parse_query_to_expression(query, context)?;

        // Add query to working memory
        self.add_fact_to_memory("user_query", vec![query_expr], 1.0)?;

        // Add deterministic query facts
        self.add_fact_to_memory(
            "user_query_text",
            vec![SymbolicExpression::Literal(LiteralValue::String(query.to_string()))],
            1.0,
        )?;

        // Add context key/value facts
        for (key, value) in context.iter() {
            let lit = match value {
                serde_json::Value::String(s) => SymbolicExpression::Literal(LiteralValue::String(s.clone())),
                serde_json::Value::Number(n) => SymbolicExpression::Literal(LiteralValue::Number(n.as_f64().unwrap_or(0.0))),
                serde_json::Value::Bool(b) => SymbolicExpression::Literal(LiteralValue::Boolean(*b)),
                _ => continue,
            };

            self.add_fact_to_memory(
                "user_context",
                vec![
                    SymbolicExpression::Literal(LiteralValue::String(key.clone())),
                    lit,
                ],
                1.0,
            )?;
        }

        // Add available tools to working memory
        for tool in available_tools {
            self.add_tool_state_to_memory(tool)?;
        }

        // Run inference
        let inferences = self.rule_engine.forward_chain(&self.rules, &mut self.working_memory)?;

        // Extract tool selections from inferences
        let mut selections = Vec::new();

        for inference in inferences {
            if let SymbolicExpression::Fact(fact) = inference {
                if fact.predicate == "tool_selected" {
                    if let (
                        Some(SymbolicExpression::Literal(LiteralValue::String(tool_name))),
                        Some(SymbolicExpression::Literal(LiteralValue::Number(confidence))),
                        Some(SymbolicExpression::Literal(LiteralValue::String(reasoning))),
                    ) = (
                        fact.arguments.get(0),
                        fact.arguments.get(1),
                        fact.arguments.get(2),
                    ) {
                        // Resolve the tool name to a concrete ToolRecord so we can attach a RecordId.
                        if let Some(tool_rec) =
                            available_tools.iter().find(|t| &t.name == tool_name)
                        {
                            selections.push(ToolSelection {
                                tool_id: tool_rec.id.clone(),
                                tool_name: tool_rec.name.clone(),
                                service_id: tool_rec.service_id.clone(),
                                confidence: *confidence as f32,
                                reasoning: reasoning.clone(),
                                dependencies: vec![],
                                estimated_cost: None,
                            });
                        }
                    }
                }
            }
        }

        // Sort by confidence and return
        selections.sort_by(|a, b| b.confidence.partial_cmp(&a.confidence).unwrap());
        Ok(selections)
    }

    /// Plan a sequence of tools to achieve a goal, subject to constraints.
    ///
    /// This constructs a `PlanningProblem` and uses backward chaining to derive
    /// an abstract `ToolPlan`. The current implementation is intentionally
    /// simplified and should be treated as a scaffold for more advanced planning.
    pub async fn plan_tool_sequence(
        &mut self,
        goal: &str,
        available_tools: &[crate::db::schema::ToolRecord],
        constraints: &PlanningConstraints,
    ) -> Result<ToolPlan> {
        // Create planning problem
        let problem = PlanningProblem {
            goal: goal.to_string(),
            available_tools: available_tools.to_vec(),
            constraints: constraints.clone(),
            // working_memory: self.working_memory.clone(),
        };

        // Use backward chaining to find plan
        let plan = self.rule_engine.backward_chain(&problem, &self.rules)?;

        Ok(plan)
    }

    /// Plan a sequence of tools for a natural-language goal using the existing
    /// backward-chaining planner and a set of default planning constraints.
    ///
    /// This is a convenience wrapper around `plan_tool_sequence` that:
    /// - uses the goal string directly as the planning goal
    /// - applies `PlanningConstraints::default()` unless explicit constraints
    ///   are provided
    /// - returns `Ok(None)` if no concrete steps could be constructed
    pub async fn plan_tools_for_goal(
        &mut self,
        goal: &str,
        available_tools: &[crate::db::schema::ToolRecord],
        constraints: Option<PlanningConstraints>,
    ) -> Result<Option<ToolPlan>> {
        let constraints = constraints.unwrap_or_else(PlanningConstraints::default);

        let plan = self
            .plan_tool_sequence(goal, available_tools, &constraints)
            .await?;

        if plan.steps.is_empty() {
            Ok(None)
        } else {
            Ok(Some(plan))
        }
    }

    fn add_fact_to_memory(&mut self, predicate: &str, arguments: Vec<SymbolicExpression>, confidence: f32) -> Result<()> {
        let fact = Fact {
            predicate: predicate.to_string(),
            arguments,
            confidence: Some(confidence),
        };
        self.working_memory
            .facts
            .entry(predicate.to_string())
            .or_default()
            .push(fact);
        Ok(())
    }

    fn add_tool_state_to_memory(&mut self, tool: &crate::db::schema::ToolRecord) -> Result<()> {
        let tool_state = ToolState {
            tool_id: tool.id.clone(),
            status: ToolStatus::Available,
            last_output: None,
            input_requirements: tool.input_schema.clone(),
            execution_count: tool.usage_count as u32,
            success_rate: 1.0, // Would need to track this separately
        };

        self.working_memory.tool_states.insert(tool.name.clone(), tool_state);

        // Add tool properties as facts
        self.add_fact_to_memory(
            "tool_exists",
            vec![
                SymbolicExpression::Literal(LiteralValue::String(tool.name.clone()))
            ],
            1.0,
        )?;

        if let Some(input_ty) = &tool.input_ty {
            self.add_fact_to_memory(
                "tool_input_type",
                vec![
                    SymbolicExpression::Literal(LiteralValue::String(tool.name.clone())),
                    SymbolicExpression::Literal(LiteralValue::String(input_ty.schema_type.clone()))
                ],
                1.0,
            )?;
        }

        if let Some(output_ty) = &tool.output_ty {
            self.add_fact_to_memory(
                "tool_output_type",
                vec![
                    SymbolicExpression::Literal(LiteralValue::String(tool.name.clone())),
                    SymbolicExpression::Literal(LiteralValue::String(output_ty.schema_type.clone()))
                ],
                1.0,
            )?;
        }

        // Add service association fact
        self.add_fact_to_memory(
            "tool_service",
            vec![
                SymbolicExpression::Literal(LiteralValue::String(tool.name.clone())),
                SymbolicExpression::Literal(LiteralValue::String(tool.service_id.to_string())),
            ],
            1.0,
        )?;

        // Add usage metrics
        self.add_fact_to_memory(
            "tool_usage",
            vec![
                SymbolicExpression::Literal(LiteralValue::String(tool.name.clone())),
                SymbolicExpression::Literal(LiteralValue::Number(tool.usage_count as f64)),
                // TODO:
                // SymbolicExpression::Literal(LiteralValue::Number(tool.success_rate.unwrap_or(1.0))),
            ],
            1.0,
        )?;

        // // TODO: Add auth requirement fact if present
        // if let Some(req) = &tool.requires_auth {
        //     self.add_fact_to_memory(
        //         "tool_requires_auth",
        //         vec![
        //             SymbolicExpression::Literal(LiteralValue::String(tool.name.clone())),
        //             SymbolicExpression::Literal(LiteralValue::Boolean(*req)),
        //         ],
        //         1.0,
        //     )?;
        // }

        Ok(())
    }

    fn parse_query_to_expression(
        &self,
        query: &str,
        _context: &HashMap<String, serde_json::Value>,
    ) -> Result<SymbolicExpression> {
        // For now we avoid any heuristic or "auto-magic" parsing here. The
        // symbolic layer should reason over explicit, deterministic facts
        // rather than guessing intent from keywords. We therefore represent
        // the user query as a single literal string expression and let rules
        // decide how (or whether) to interpret it.
        Ok(SymbolicExpression::Literal(LiteralValue::String(
            query.to_string(),
        )))
    }
}

/// Hard constraints and preferences for planning a tool sequence.
#[derive(Debug, Clone)]
pub struct PlanningConstraints {
    pub max_steps: u32,
    pub timeout_seconds: u32, // TODO
    pub allowed_tools: Option<Vec<String>>,
    pub forbidden_tools: Option<Vec<String>>,
    pub max_cost: Option<f32>,
    pub requirements: Vec<String>,
}

impl Default for PlanningConstraints {
    fn default() -> Self {
        Self {
            max_steps: 8,
            timeout_seconds: 30,
            allowed_tools: None,
            forbidden_tools: None,
            max_cost: None,
            requirements: Vec::new(),
        }
    }
}

/// A planning problem instance presented to the rule engine.
#[derive(Debug, Clone)]
pub struct PlanningProblem {
    pub goal: String,
    pub available_tools: Vec<crate::db::schema::ToolRecord>,
    pub constraints: PlanningConstraints,
    // pub working_memory: WorkingMemory, // TODO
}

/// A single tool candidate proposed by the symbolic engine for a given query.
///
/// The `tool_id` here is the database identifier (`RecordId`) of the tool,
/// so callers can directly map a selection to a concrete `ToolRecord`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSelection {
    pub tool_id: RecordId,
    pub tool_name: String,
    pub service_id: RecordId,
    pub confidence: f32,
    pub reasoning: String,
    pub dependencies: Vec<String>,
    pub estimated_cost: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolPlan {
    pub id: String,
    pub goal: String,
    pub steps: Vec<PlanStep>,
    pub estimated_cost: f32,
    pub estimated_time: f32,
    pub confidence: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    pub step_number: u32,
    pub tool_id: RecordId,
    pub inputs: HashMap<String, serde_json::Value>,
    pub expected_outputs: Vec<String>,
    pub parallel: bool,
    pub dependencies: Vec<u32>,
}

/// Rule engine implementation for forward and backward chaining over symbolic rules.
///
/// It currently supports facts and basic boolean connectives. Quantifiers,
/// comparisons, and full unification are not yet implemented and can be layered
/// on top of this core.
struct RuleEngine;

impl RuleEngine {
    fn new() -> Self {
        Self
    }

    fn forward_chain(
        &self,
        rules: &[SymbolicRule],
        memory: &mut WorkingMemory,
    ) -> Result<Vec<SymbolicExpression>> {
        let mut new_facts = Vec::new();
        let mut changed = true;

        while changed {
            changed = false;

            for rule in rules {
                // Special case: if the rule has a single fact antecedent, we try to
                // perform simple variable unification and instantiate the consequents
                // for each matching binding.
                if rule.antecedents.len() == 1 {
                    if let SymbolicExpression::Fact(pattern_fact) = &rule.antecedents[0] {
                        if let Some(existing_facts) = memory.facts.get(&pattern_fact.predicate) {
                            // Clone the facts so we don't hold an immutable borrow on
                            // `memory.facts` while we potentially insert new facts into it.
                            let existing_facts = existing_facts.clone();
                            for concrete in &existing_facts {
                                if let Some(bindings) = self.unify_fact(pattern_fact, concrete) {
                                    for consequent in &rule.consequents {
                                        if let SymbolicExpression::Fact(cf) = consequent {
                                            let instantiated = self.substitute_fact(cf, &bindings);

                                            let entry = memory
                                                .facts
                                                .entry(instantiated.predicate.clone())
                                                .or_default();

                                            let exists = entry
                                                .iter()
                                                .any(|existing| self.facts_match(&instantiated, existing));

                                            if !exists {
                                                entry.push(instantiated.clone());
                                                new_facts.push(SymbolicExpression::Fact(instantiated));
                                                changed = true;
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        // We've handled this rule via unification; move to next rule.
                        continue;
                    }
                }

                // Default behaviour: boolean-style evaluation of antecedents without
                // variable binding. This supports existing rules that use only
                // literal arguments.
                if self.evaluate_antecedents(&rule.antecedents, memory)? {
                    for consequent in &rule.consequents {
                        if let SymbolicExpression::Fact(fact) = consequent {
                            let entry = memory
                                .facts
                                .entry(fact.predicate.clone())
                                .or_default();

                            let exists = entry
                                .iter()
                                .any(|existing| self.facts_match(fact, existing));

                            if !exists {
                                entry.push(fact.clone());
                                new_facts.push(SymbolicExpression::Fact(fact.clone()));
                                changed = true;
                            }
                        }
                    }
                }
            }
        }

        Ok(new_facts)
    }

    fn backward_chain(
        &self,
        problem: &PlanningProblem,
        rules: &[SymbolicRule],
    ) -> Result<ToolPlan> {
        // Simplified backward chaining implementation
        let mut steps = Vec::new();
        let mut goal_stack = vec![problem.goal.clone()];
        let mut step_number = 0;

        while !goal_stack.is_empty() && step_number < problem.constraints.max_steps {
            let current_goal = goal_stack.pop().unwrap();

            // Find rules that can achieve this goal
            'rules: for rule in rules {
                if self.can_achieve_goal(rule, &current_goal) {
                    // Try to extract a tool name from the rule (e.g. via a `use_tool("name")` fact)
                    let tool_name_opt = self.extract_tool_name_from_rule(rule);

                    // Apply planning constraints based on tool name, if we have one.
                    if let Some(tool_name) = &tool_name_opt {
                        if let Some(allowed) = &problem.constraints.allowed_tools {
                            if !allowed.contains(tool_name) {
                                continue 'rules;
                            }
                        }
                        if let Some(forbidden) = &problem.constraints.forbidden_tools {
                            if forbidden.contains(tool_name) {
                                continue 'rules;
                            }
                        }
                    }

                    let next_step_number = step_number + 1;

                    // Create step from rule; if we fail to derive a concrete tool, skip this rule.
                    match self.create_plan_step(rule, next_step_number, &current_goal, problem) {
                        Ok(step) => {
                            step_number = next_step_number;
                            steps.push(step);

                            // Add new sub-goals from rule antecedents
                            for antecedent in &rule.antecedents {
                                if let SymbolicExpression::Fact(fact) = antecedent {
                                    if fact.predicate.starts_with("require_") {
                                        goal_stack.push(fact.predicate.clone());
                                    }
                                }
                            }

                            break 'rules;
                        }
                        Err(_) => {
                            // Could not create a concrete step from this rule; try the next rule.
                            continue 'rules;
                        }
                    }
                }
            }
        }

        Ok(ToolPlan {
            id: uuid::Uuid::new_v4().to_string(),
            goal: problem.goal.clone(),
            steps,
            estimated_cost: 0.0,
            estimated_time: 0.0,
            confidence: 0.8,
        })
    }

    /// Attempt to extract the name of a tool referenced by this rule.
    ///
    /// By convention, planning-related rules should include a consequent or
    /// antecedent of the form:
    ///     use_tool("tool_name")
    /// so that the planner can map this rule to a concrete tool.
    fn extract_tool_name_from_rule(&self, rule: &SymbolicRule) -> Option<String> {
        let exprs = rule
            .consequents
            .iter()
            .chain(rule.antecedents.iter());

        for expr in exprs {
            if let SymbolicExpression::Fact(Fact { predicate, arguments, .. }) = expr {
                if predicate == "use_tool" {
                    if let Some(SymbolicExpression::Literal(LiteralValue::String(name))) =
                        arguments.get(0)
                    {
                        return Some(name.clone());
                    }
                }
            }
        }

        None
    }

    fn evaluate_antecedents(
        &self,
        antecedents: &[SymbolicExpression],
        memory: &WorkingMemory,
    ) -> Result<bool> {
        for antecedent in antecedents {
            if !self.evaluate_expression(antecedent, memory)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn evaluate_expression(
        &self,
        expr: &SymbolicExpression,
        memory: &WorkingMemory,
    ) -> Result<bool> {
        match expr {
            SymbolicExpression::Fact(fact) => {
                if let Some(existing) = memory.facts.get(&fact.predicate) {
                    Ok(existing.iter().any(|f| self.facts_match(fact, f)))
                } else {
                    Ok(false)
                }
            }
            SymbolicExpression::And(exprs) => {
                for e in exprs {
                    if !self.evaluate_expression(e, memory)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            SymbolicExpression::Or(exprs) => {
                for e in exprs {
                    if self.evaluate_expression(e, memory)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            SymbolicExpression::Not(expr) => {
                Ok(!self.evaluate_expression(expr, memory)?)
            }
            _ => Ok(true), // Non-fact expressions are treated as true for now.
        }
    }

    fn facts_match(&self, pattern: &Fact, concrete: &Fact) -> bool {
        if pattern.predicate != concrete.predicate {
            return false;
        }
        if pattern.arguments.len() != concrete.arguments.len() {
            return false;
        }

        for (pa, ca) in pattern.arguments.iter().zip(&concrete.arguments) {
            match (pa, ca) {
                (
                    SymbolicExpression::Literal(LiteralValue::String(ps)),
                    SymbolicExpression::Literal(LiteralValue::String(cs)),
                ) if ps != cs => return false,
                (
                    SymbolicExpression::Literal(LiteralValue::Number(pn)),
                    SymbolicExpression::Literal(LiteralValue::Number(cn)),
                ) if pn != cn => return false,
                (
                    SymbolicExpression::Literal(LiteralValue::Boolean(pb)),
                    SymbolicExpression::Literal(LiteralValue::Boolean(cb)),
                ) if pb != cb => return false,
                _ => {
                    // For now, treat non-literal or mismatched types as "don't care".
                }
            }
        }

        true
    }

    /// Attempt to unify a pattern fact with a concrete fact, producing a simple
    /// variable binding environment if successful. This supports rules of the form
    /// `tool_exists(T) => tool_selected(T, ...)`.
    fn unify_fact(
        &self,
        pattern: &Fact,
        concrete: &Fact,
    ) -> Option<HashMap<String, LiteralValue>> {
        if pattern.predicate != concrete.predicate {
            return None;
        }
        if pattern.arguments.len() != concrete.arguments.len() {
            return None;
        }

        let mut bindings: HashMap<String, LiteralValue> = HashMap::new();

        for (pa, ca) in pattern.arguments.iter().zip(&concrete.arguments) {
            match (pa, ca) {
                (SymbolicExpression::Variable(name), SymbolicExpression::Literal(lit)) => {
                    // If the variable was already bound, ensure it is consistent.
                    if let Some(existing) = bindings.get(name) {
                        if existing != lit {
                            return None;
                        }
                    } else {
                        bindings.insert(name.clone(), lit.clone());
                    }
                }
                (
                    SymbolicExpression::Literal(LiteralValue::String(ps)),
                    SymbolicExpression::Literal(LiteralValue::String(cs)),
                ) if ps != cs => return None,
                (
                    SymbolicExpression::Literal(LiteralValue::Number(pn)),
                    SymbolicExpression::Literal(LiteralValue::Number(cn)),
                ) if pn != cn => return None,
                (
                    SymbolicExpression::Literal(LiteralValue::Boolean(pb)),
                    SymbolicExpression::Literal(LiteralValue::Boolean(cb)),
                ) if pb != cb => return None,
                // For now, treat other combinations as "don't care".
                _ => {}
            }
        }

        Some(bindings)
    }

    /// Instantiate a fact by substituting any `Variable` arguments using the provided
    /// bindings. Arguments that are not variables are cloned as-is.
    fn substitute_fact(
        &self,
        fact: &Fact,
        bindings: &HashMap<String, LiteralValue>,
    ) -> Fact {
        let mut new_args = Vec::with_capacity(fact.arguments.len());

        for arg in &fact.arguments {
            match arg {
                SymbolicExpression::Variable(name) => {
                    if let Some(v) = bindings.get(name) {
                        new_args.push(SymbolicExpression::Literal(v.clone()));
                    } else {
                        // If we don't have a binding, keep it as a variable.
                        new_args.push(arg.clone());
                    }
                }
                _ => new_args.push(arg.clone()),
            }
        }

        Fact {
            predicate: fact.predicate.clone(),
            arguments: new_args,
            confidence: fact.confidence,
        }
    }

    /// Check if this rule can help achieve the given goal, based purely on its consequents.
    ///
    /// Higher-level filtering based on planning constraints and available tools is handled
    /// by the caller (e.g. in `backward_chain`), so this function answers only the question:
    /// "does this rule conceptually relate to the goal?".
    fn can_achieve_goal(
        &self,
        rule: &SymbolicRule,
        goal: &str,
    ) -> bool {
        // Check if rule's consequents can achieve the goal
        for consequent in &rule.consequents {
            if let SymbolicExpression::Fact(fact) = consequent {
                if fact.predicate.contains(goal) || goal.contains(&fact.predicate) {
                    return true;
                }
            }
        }
        false
    }

    fn create_plan_step(
        &self,
        rule: &SymbolicRule,
        step_number: u32,
        goal: &str,
        problem: &PlanningProblem,
    ) -> Result<PlanStep> {
        // Derive a tool name from the rule.
        let tool_name = self
            .extract_tool_name_from_rule(rule)
            .ok_or_else(|| anyhow::Error::msg(format!(
                "Rule '{}' has no use_tool(\"...\") fact to derive a tool for planning",
                rule.name
            )))?;

        // Look up the concrete tool in the planning problem.
        let tool = problem
            .available_tools
            .iter()
            .find(|t| t.name == tool_name)
            .ok_or_else(|| anyhow::Error::msg(format!(
                "No tool found with name '{}' for planning",
                tool_name
            )))?;

        Ok(PlanStep {
            step_number,
            tool_id: tool.id.clone(),
            inputs: HashMap::new(),
            expected_outputs: vec![goal.to_string()],
            parallel: false,
            dependencies: vec![],
        })
    }
}
