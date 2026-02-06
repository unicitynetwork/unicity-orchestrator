# Symbolic Reasoning

The symbolic reasoning engine adds rule-based inference on top of semantic search, enabling the orchestrator to make logical deductions about tool selection and chaining.

## Rules

Symbolic rules are stored in the `symbolic_rule` database table and follow an antecedent-consequent pattern:

```rust
pub struct SymbolicRule {
    pub name: String,
    pub description: String,
    pub antecedents: Vec<SymbolicExpression>,
    pub consequents: Vec<SymbolicExpression>,
    pub confidence: f64,
    pub priority: i32,
}
```

- **Antecedents** — Conditions that must be satisfied
- **Consequents** — Facts that are inferred when conditions are met
- **Confidence** — How certain the inference is (0.0 to 1.0)
- **Priority** — Execution order when multiple rules match

## Expressions

The rule engine supports a rich expression language:

| Expression | Description |
|------------|-------------|
| `Fact { predicate, arguments, confidence }` | A typed predicate with arguments |
| `And(expressions)` | All sub-expressions must hold |
| `Or(expressions)` | At least one sub-expression must hold |
| `Not(expression)` | Negation |
| `Implies(lhs, rhs)` | Logical implication |
| `Quantified { quantifier, variable, expression }` | Universal/existential quantification |
| `Comparison { operator, left, right }` | Comparison operators |
| `Variable(name)` | Unbound variable for unification |
| `Literal(value)` | Concrete JSON value |

## Working Memory

The rule engine operates on a working memory that tracks the current state:

```rust
pub struct WorkingMemory {
    pub facts: HashMap<String, Vec<Fact>>,
    pub variables: HashMap<String, serde_json::Value>,
    pub tool_states: HashMap<String, ToolState>,
}
```

Tool states track execution status:

| Status | Description |
|--------|-------------|
| `Available` | Tool is ready to execute |
| `Executing` | Tool is currently running |
| `Completed` | Tool finished successfully |
| `Failed` | Tool execution failed |
| `Blocked` | Tool is blocked by dependencies |

## Inference Strategies

### Forward Chaining

Starts from known facts and applies rules to derive new facts. Used during tool selection to augment semantic search results with logical inferences.

The engine includes **variable unification and substitution** — variables in rule templates are bound to concrete values when matched against the working memory.

### Backward Chaining

Starts from a goal and works backward to find rules that could achieve it. Used for planning multi-step tool chains.

## Example Rule

This rule suggests a data processing tool when a file read tool is selected:

```rust
SymbolicRule {
    name: "File Operation Chain".to_string(),
    description: "Chain file read with data processing".to_string(),
    antecedents: vec![
        SymbolicExpression::Fact(Fact {
            predicate: "tool_selected".to_string(),
            arguments: vec![
                SymbolicExpression::Variable("tool".to_string()),
                SymbolicExpression::Literal(json!("file_read")),
            ],
            confidence: Some(0.9),
        })
    ],
    consequents: vec![
        SymbolicExpression::Fact(Fact {
            predicate: "suggest_following_tool".to_string(),
            arguments: vec![
                SymbolicExpression::Variable("following_tool".to_string()),
                SymbolicExpression::Literal(json!("data_parse")),
            ],
            confidence: Some(0.8),
        })
    ],
    confidence: 0.85,
    priority: 100,
}
```

## Integration with Query Pipeline

During `query_tools`, the symbolic reasoner runs after semantic search:

1. Semantic search populates the working memory with candidate tools
2. Forward chaining derives additional facts (e.g., "suggest following tool")
3. Results are merged — symbolic suggestions boost or add to the semantic results
4. If symbolic reasoning yields no results, raw embedding results are used as fallback
