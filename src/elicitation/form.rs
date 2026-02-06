//! Form mode elicitation handler.
//!
//! Form mode allows servers to collect structured data through the MCP client
//! using JSON Schema for validation. This module works with rmcp's type-safe
//! `ElicitationSchema` for request construction and provides response validation.

use crate::elicitation::{ElicitationError, ElicitationResult, ElicitationAction, ElicitationSchema, CreateElicitationResult, StringFormat};
use rmcp::model::{EnumSchema, SingleSelectEnumSchema, MultiSelectEnumSchema};
use serde_json::Value;
use url::Url;

/// Extract valid enum values from an EnumSchema (handles all variants).
fn get_enum_values(enum_schema: &EnumSchema) -> Vec<&str> {
    match enum_schema {
        EnumSchema::Single(single) => match single {
            SingleSelectEnumSchema::Untitled(s) => s.enum_.iter().map(|v| v.as_str()).collect(),
            SingleSelectEnumSchema::Titled(s) => s.one_of.iter().map(|c| c.const_.as_ref()).collect(),
        },
        EnumSchema::Multi(multi) => match multi {
            MultiSelectEnumSchema::Untitled(s) => s.items.enum_.iter().map(|v| v.as_str()).collect(),
            MultiSelectEnumSchema::Titled(s) => s.items.any_of.iter().map(|c| c.const_.as_ref()).collect(),
        },
        EnumSchema::Legacy(legacy) => legacy.enum_.iter().map(|v| v.as_str()).collect(),
    }
}

/// Form mode elicitation handler.
///
/// Provides validation utilities for elicitation responses.
/// Request schema construction is handled by rmcp's type-safe `ElicitationSchema`.
#[derive(Clone)]
pub struct FormHandler;

impl FormHandler {
    /// Create a new form handler.
    pub fn new() -> Self {
        Self
    }

    /// Validate a response against the expected schema.
    ///
    /// This validates that the client's response conforms to the schema
    /// that was sent in the elicitation request.
    pub fn validate_response(
        &self,
        schema: &ElicitationSchema,
        response: &CreateElicitationResult,
    ) -> ElicitationResult<Value> {
        match response.action {
            ElicitationAction::Accept => {
                let content = response.content.as_ref()
                    .ok_or_else(|| ElicitationError::InvalidSchema("Response must include content".to_string()))?;

                // Validate content against schema
                self.validate_content_against_schema(schema, content)?;

                Ok(content.clone())
            }
            ElicitationAction::Decline => Err(ElicitationError::Declined),
            ElicitationAction::Cancel => Err(ElicitationError::Canceled),
        }
    }

    /// Validate content against an ElicitationSchema.
    fn validate_content_against_schema(
        &self,
        schema: &ElicitationSchema,
        content: &Value,
    ) -> ElicitationResult<()> {
        let content_obj = content.as_object()
            .ok_or_else(|| ElicitationError::InvalidSchema("Content must be an object".to_string()))?;

        // Check required fields
        if let Some(required) = &schema.required {
            for req_field in required {
                if !content_obj.contains_key(req_field) {
                    return Err(ElicitationError::InvalidSchema(format!("Missing required field: {}", req_field)));
                }
            }
        }

        // Validate each field in content against its schema
        for (key, value) in content_obj {
            if let Some(prop_schema) = schema.properties.get(key) {
                self.validate_value_against_primitive(key, value, prop_schema)?;
            }
            // Additional fields are allowed (JSON Schema default behavior)
        }

        Ok(())
    }

    /// Validate a value against a PrimitiveSchema.
    fn validate_value_against_primitive(
        &self,
        name: &str,
        value: &Value,
        schema: &crate::elicitation::PrimitiveSchema,
    ) -> ElicitationResult<()> {
        use crate::elicitation::PrimitiveSchema;

        match schema {
            PrimitiveSchema::String(string_schema) => {
                let s = value.as_str()
                    .ok_or_else(|| ElicitationError::InvalidSchema(format!("Property '{}' must be a string", name)))?;

                // Check length constraints
                if let Some(min_len) = string_schema.min_length {
                    if s.len() < min_len as usize {
                        return Err(ElicitationError::InvalidSchema(
                            format!("Property '{}' is too short (min: {})", name, min_len)
                        ));
                    }
                }
                if let Some(max_len) = string_schema.max_length {
                    if s.len() > max_len as usize {
                        return Err(ElicitationError::InvalidSchema(
                            format!("Property '{}' is too long (max: {})", name, max_len)
                        ));
                    }
                }

                // Validate format if specified
                if let Some(format) = &string_schema.format {
                    self.validate_string_format(name, s, format)?;
                }
            }
            PrimitiveSchema::Number(number_schema) => {
                let n = value.as_f64()
                    .ok_or_else(|| ElicitationError::InvalidSchema(format!("Property '{}' must be a number", name)))?;

                if let Some(min) = number_schema.minimum {
                    if n < min {
                        return Err(ElicitationError::InvalidSchema(
                            format!("Property '{}' is below minimum ({})", name, min)
                        ));
                    }
                }
                if let Some(max) = number_schema.maximum {
                    if n > max {
                        return Err(ElicitationError::InvalidSchema(
                            format!("Property '{}' is above maximum ({})", name, max)
                        ));
                    }
                }
            }
            PrimitiveSchema::Integer(int_schema) => {
                let n = value.as_i64()
                    .ok_or_else(|| ElicitationError::InvalidSchema(format!("Property '{}' must be an integer", name)))?;

                if let Some(min) = int_schema.minimum {
                    if n < min {
                        return Err(ElicitationError::InvalidSchema(
                            format!("Property '{}' is below minimum ({})", name, min)
                        ));
                    }
                }
                if let Some(max) = int_schema.maximum {
                    if n > max {
                        return Err(ElicitationError::InvalidSchema(
                            format!("Property '{}' is above maximum ({})", name, max)
                        ));
                    }
                }
            }
            PrimitiveSchema::Boolean(_) => {
                if !value.is_boolean() {
                    return Err(ElicitationError::InvalidSchema(format!("Property '{}' must be a boolean", name)));
                }
            }
            PrimitiveSchema::Enum(enum_schema) => {
                let s = value.as_str()
                    .ok_or_else(|| ElicitationError::InvalidSchema(format!("Property '{}' must be a string", name)))?;

                let valid_values = get_enum_values(enum_schema);
                if !valid_values.contains(&s) {
                    return Err(ElicitationError::InvalidSchema(
                        format!("Property '{}' has invalid enum value: {}", name, s)
                    ));
                }
            }
        }

        Ok(())
    }

    /// Validate a string value against a format constraint.
    ///
    /// Validates the MCP spec formats: email, uri, date, date-time
    fn validate_string_format(&self, name: &str, value: &str, format: &StringFormat) -> ElicitationResult<()> {
        match format {
            StringFormat::Email => {
                // Simple email validation (contains @ and .)
                if !value.contains('@') || !value.contains('.') {
                    return Err(ElicitationError::InvalidSchema(
                        format!("Property '{}' is not a valid email address", name)
                    ));
                }
            }
            StringFormat::Uri => {
                // Validate URI format
                if Url::parse(value).is_err() {
                    return Err(ElicitationError::InvalidSchema(
                        format!("Property '{}' is not a valid URI", name)
                    ));
                }
            }
            StringFormat::Date => {
                // Validate ISO date format (YYYY-MM-DD)
                if chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").is_err() {
                    return Err(ElicitationError::InvalidSchema(
                        format!("Property '{}' is not a valid date (expected YYYY-MM-DD)", name)
                    ));
                }
            }
            StringFormat::DateTime => {
                // Validate ISO 8601 date-time format
                if chrono::DateTime::parse_from_rfc3339(value).is_err() {
                    return Err(ElicitationError::InvalidSchema(
                        format!("Property '{}' is not a valid date-time (expected ISO 8601)", name)
                    ));
                }
            }
        }

        Ok(())
    }
}

impl Default for FormHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_schema() -> ElicitationSchema {
        ElicitationSchema::builder()
            .required_string("name")
            .required_email("email")
            .optional_integer("age", 0, 150)
            .build()
            .unwrap()
    }

    #[test]
    fn test_validate_valid_response() {
        let handler = FormHandler::new();
        let schema = create_test_schema();

        let response = CreateElicitationResult {
            action: ElicitationAction::Accept,
            content: Some(serde_json::json!({
                "name": "John Doe",
                "email": "john@example.com",
                "age": 30
            })),
        };

        assert!(handler.validate_response(&schema, &response).is_ok());
    }

    #[test]
    fn test_validate_missing_required_field() {
        let handler = FormHandler::new();
        let schema = create_test_schema();

        let response = CreateElicitationResult {
            action: ElicitationAction::Accept,
            content: Some(serde_json::json!({
                "name": "John Doe"
                // Missing required "email" field
            })),
        };

        assert!(handler.validate_response(&schema, &response).is_err());
    }

    #[test]
    fn test_validate_invalid_email() {
        let handler = FormHandler::new();
        let schema = create_test_schema();

        let response = CreateElicitationResult {
            action: ElicitationAction::Accept,
            content: Some(serde_json::json!({
                "name": "John Doe",
                "email": "not-an-email"  // Invalid email
            })),
        };

        assert!(handler.validate_response(&schema, &response).is_err());
    }

    #[test]
    fn test_validate_integer_out_of_range() {
        let handler = FormHandler::new();
        let schema = create_test_schema();

        let response = CreateElicitationResult {
            action: ElicitationAction::Accept,
            content: Some(serde_json::json!({
                "name": "John Doe",
                "email": "john@example.com",
                "age": 200  // Out of range (max 150)
            })),
        };

        assert!(handler.validate_response(&schema, &response).is_err());
    }

    #[test]
    fn test_validate_decline_returns_error() {
        let handler = FormHandler::new();
        let schema = create_test_schema();

        let response = CreateElicitationResult {
            action: ElicitationAction::Decline,
            content: None,
        };

        let result = handler.validate_response(&schema, &response);
        assert!(matches!(result, Err(ElicitationError::Declined)));
    }

    #[test]
    fn test_validate_cancel_returns_error() {
        let handler = FormHandler::new();
        let schema = create_test_schema();

        let response = CreateElicitationResult {
            action: ElicitationAction::Cancel,
            content: None,
        };

        let result = handler.validate_response(&schema, &response);
        assert!(matches!(result, Err(ElicitationError::Canceled)));
    }

    #[test]
    fn test_validate_enum_values() {
        use rmcp::model::EnumSchema as RmcpEnumSchema;
        let handler = FormHandler::new();
        let schema = ElicitationSchema::builder()
            .required_enum_schema("color", RmcpEnumSchema::builder(vec!["red".to_string(), "green".to_string(), "blue".to_string()]).build())
            .build()
            .unwrap();

        // Valid enum value
        let valid_response = CreateElicitationResult {
            action: ElicitationAction::Accept,
            content: Some(serde_json::json!({"color": "red"})),
        };
        assert!(handler.validate_response(&schema, &valid_response).is_ok());

        // Invalid enum value
        let invalid_response = CreateElicitationResult {
            action: ElicitationAction::Accept,
            content: Some(serde_json::json!({"color": "yellow"})),
        };
        assert!(handler.validate_response(&schema, &invalid_response).is_err());
    }
}
