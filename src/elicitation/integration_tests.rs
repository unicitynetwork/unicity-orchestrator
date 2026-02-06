//! Integration tests for elicitation between server and client.
//!
//! These tests verify the full elicitation flow using duplex streams
//! to connect a test server and client.

#![cfg(test)]

use std::future::Future;
use std::sync::Arc;
use tokio::sync::RwLock;

use rmcp::{
    ErrorData as McpError,
    handler::{client::ClientHandler, server::ServerHandler},
    model::*,
    service::{NotificationContext, Peer, RequestContext, RoleClient, RoleServer, ServiceExt},
};

/// Test client that accepts elicitation requests with a configured response.
struct TestClient {
    /// The response to return when elicitation is requested
    response_action: ElicitationAction,
    /// Content to return (for Accept action)
    response_content: Option<serde_json::Value>,
    /// Track received elicitations for assertions
    received_elicitations: Arc<RwLock<Vec<CreateElicitationRequestParams>>>,
}

impl TestClient {
    fn new(action: ElicitationAction, content: Option<serde_json::Value>) -> Self {
        Self {
            response_action: action,
            response_content: content,
            received_elicitations: Arc::new(RwLock::new(Vec::new())),
        }
    }

    fn accepting(content: serde_json::Value) -> Self {
        Self::new(ElicitationAction::Accept, Some(content))
    }

    fn declining() -> Self {
        Self::new(ElicitationAction::Decline, None)
    }

    fn received_elicitations(&self) -> Arc<RwLock<Vec<CreateElicitationRequestParams>>> {
        self.received_elicitations.clone()
    }
}

impl ClientHandler for TestClient {
    fn get_info(&self) -> ClientInfo {
        ClientInfo {
            meta: None,
            protocol_version: ProtocolVersion::V_2025_06_18,
            capabilities: ClientCapabilities::builder().enable_elicitation().build(),
            client_info: Implementation {
                name: "test-client".into(),
                version: "1.0.0".into(),
                title: None,
                icons: None,
                website_url: None,
            },
        }
    }

    fn create_elicitation(
        &self,
        request: CreateElicitationRequestParams,
        _context: RequestContext<RoleClient>,
    ) -> impl Future<Output = Result<CreateElicitationResult, McpError>> + Send + '_ {
        let action = self.response_action.clone();
        let content = self.response_content.clone();
        let received = self.received_elicitations.clone();

        async move {
            // Store the received request for later assertions
            received.write().await.push(request);

            Ok(CreateElicitationResult { action, content })
        }
    }

    fn on_cancelled(
        &self,
        _notification: CancelledNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) -> impl Future<Output = ()> + Send + '_ {
        std::future::ready(())
    }

    fn on_progress(
        &self,
        _notification: ProgressNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) -> impl Future<Output = ()> + Send + '_ {
        std::future::ready(())
    }
}

/// Minimal test server that can send elicitations.
struct TestServer {
    /// Stored peer for sending elicitations
    peer: Arc<RwLock<Option<Peer<RoleServer>>>>,
}

impl TestServer {
    fn new() -> Self {
        Self {
            peer: Arc::new(RwLock::new(None)),
        }
    }

    async fn send_elicitation(
        &self,
        message: &str,
        schema: ElicitationSchema,
    ) -> Result<CreateElicitationResult, String> {
        let peer_guard = self.peer.read().await;
        let peer = peer_guard.as_ref().ok_or("No peer connected")?;

        let params = CreateElicitationRequestParams {
            message: message.to_string(),
            requested_schema: schema,
            meta: None,
        };

        peer.create_elicitation(params)
            .await
            .map_err(|e| format!("Elicitation failed: {:?}", e))
    }
}

impl ServerHandler for TestServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2025_06_18,
            capabilities: ServerCapabilities::builder().build(),
            server_info: Implementation {
                name: "test-server".into(),
                version: "1.0.0".into(),
                title: None,
                icons: None,
                website_url: None,
            },
            instructions: None,
        }
    }

    fn initialize(
        &self,
        _request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<InitializeResult, McpError>> + Send + '_ {
        let peer_storage = self.peer.clone();
        let peer = context.peer.clone();

        async move {
            // Store the peer for later use
            *peer_storage.write().await = Some(peer);

            Ok(InitializeResult {
                protocol_version: ProtocolVersion::V_2025_06_18,
                capabilities: ServerCapabilities::builder().build(),
                server_info: Implementation {
                    name: "test-server".into(),
                    version: "1.0.0".into(),
                    title: None,
                    icons: None,
                    website_url: None,
                },
                instructions: None,
            })
        }
    }

    fn ping(
        &self,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<(), McpError>> + Send + '_ {
        std::future::ready(Ok(()))
    }

    fn on_cancelled(
        &self,
        _notification: CancelledNotificationParam,
        _context: NotificationContext<RoleServer>,
    ) -> impl Future<Output = ()> + Send + '_ {
        std::future::ready(())
    }

    fn on_progress(
        &self,
        _notification: ProgressNotificationParam,
        _context: NotificationContext<RoleServer>,
    ) -> impl Future<Output = ()> + Send + '_ {
        std::future::ready(())
    }

    fn on_initialized(
        &self,
        _context: NotificationContext<RoleServer>,
    ) -> impl Future<Output = ()> + Send + '_ {
        std::future::ready(())
    }

    fn on_roots_list_changed(
        &self,
        _context: NotificationContext<RoleServer>,
    ) -> impl Future<Output = ()> + Send + '_ {
        std::future::ready(())
    }
}

#[tokio::test]
async fn test_elicitation_accept_flow() {
    // Create duplex streams for bidirectional communication
    let (client_stream, server_stream) = tokio::io::duplex(4096);

    // Create server and client
    let server = TestServer::new();
    let client = TestClient::accepting(serde_json::json!({
        "action": "allow_once"
    }));
    let received_elicitations = client.received_elicitations();

    // Split the streams for proper direction
    let (server_read, server_write) = tokio::io::split(server_stream);
    let (client_read, client_write) = tokio::io::split(client_stream);

    // Start server (reads from server_read, writes to server_write)
    let server_handle = tokio::spawn(async move {
        let running = server.serve((server_read, server_write)).await.unwrap();

        // Wait a bit for the client to initialize, then send elicitation
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let schema = ElicitationSchema::builder()
            .required_enum_schema(
                "action",
                EnumSchema::builder(vec![
                    "allow_once".to_string(),
                    "always_allow".to_string(),
                    "deny".to_string(),
                ])
                .build(),
            )
            .build()
            .unwrap();

        let result = running
            .service()
            .send_elicitation("Please approve this tool execution", schema)
            .await;

        (running, result)
    });

    // Start client (reads from client_read, writes to client_write)
    let client_handle =
        tokio::spawn(async move { client.serve((client_read, client_write)).await });

    // Wait for both with timeout
    let result = tokio::time::timeout(tokio::time::Duration::from_secs(5), async {
        let (server_result, _client_result) = tokio::join!(server_handle, client_handle);
        server_result
    })
    .await;

    match result {
        Ok(Ok((_, Ok(elicitation_result)))) => {
            // Verify the response
            assert_eq!(elicitation_result.action, ElicitationAction::Accept);
            assert!(elicitation_result.content.is_some());

            let content = elicitation_result.content.unwrap();
            assert_eq!(
                content
                    .get("action")
                    .and_then(|v: &serde_json::Value| v.as_str()),
                Some("allow_once")
            );

            // Verify the client received the elicitation
            let received = received_elicitations.read().await;
            assert_eq!(received.len(), 1);
            assert!(received[0].message.contains("approve this tool"));
        }
        Ok(Ok((_, Err(e)))) => panic!("Elicitation failed: {}", e),
        Ok(Err(e)) => panic!("Server task failed: {:?}", e),
        Err(_) => panic!("Test timed out"),
    }
}

#[tokio::test]
async fn test_elicitation_decline_flow() {
    let (client_stream, server_stream) = tokio::io::duplex(4096);

    let server = TestServer::new();
    let client = TestClient::declining();

    let (server_read, server_write) = tokio::io::split(server_stream);
    let (client_read, client_write) = tokio::io::split(client_stream);

    let server_handle = tokio::spawn(async move {
        let running = server.serve((server_read, server_write)).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let schema = ElicitationSchema::builder()
            .required_string("reason")
            .build()
            .unwrap();

        let result = running
            .service()
            .send_elicitation("Why do you need access?", schema)
            .await;

        (running, result)
    });

    let client_handle =
        tokio::spawn(async move { client.serve((client_read, client_write)).await });

    let result = tokio::time::timeout(tokio::time::Duration::from_secs(5), async {
        let (server_result, _) = tokio::join!(server_handle, client_handle);
        server_result
    })
    .await;

    match result {
        Ok(Ok((_, Ok(elicitation_result)))) => {
            assert_eq!(elicitation_result.action, ElicitationAction::Decline);
            assert!(elicitation_result.content.is_none());
        }
        Ok(Ok((_, Err(e)))) => panic!("Elicitation failed: {}", e),
        Ok(Err(e)) => panic!("Server task failed: {:?}", e),
        Err(_) => panic!("Test timed out"),
    }
}

#[tokio::test]
async fn test_elicitation_with_provenance() {
    use super::wrap_with_provenance;

    let (client_stream, server_stream) = tokio::io::duplex(4096);

    let server = TestServer::new();
    let client = TestClient::accepting(serde_json::json!({"approved": true}));
    let received_elicitations = client.received_elicitations();

    let (server_read, server_write) = tokio::io::split(server_stream);
    let (client_read, client_write) = tokio::io::split(client_stream);

    let server_handle = tokio::spawn(async move {
        let running = server.serve((server_read, server_write)).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Use provenance wrapping like forward_elicitation does
        let original_message = "Please enter your API key";
        let wrapped_message = wrap_with_provenance(original_message, "github");

        let schema = ElicitationSchema::builder()
            .required_string("api_key")
            .build()
            .unwrap();

        let result = running
            .service()
            .send_elicitation(&wrapped_message, schema)
            .await;

        (running, result)
    });

    let client_handle =
        tokio::spawn(async move { client.serve((client_read, client_write)).await });

    let result = tokio::time::timeout(tokio::time::Duration::from_secs(5), async {
        let (server_result, _) = tokio::join!(server_handle, client_handle);
        server_result
    })
    .await;

    match result {
        Ok(Ok((_, Ok(_)))) => {
            // Verify the client received the message with provenance
            let received = received_elicitations.read().await;
            assert_eq!(received.len(), 1);

            // Message should be wrapped with service name
            assert!(received[0].message.starts_with("[github]"));
            assert!(received[0].message.contains("API key"));
        }
        Ok(Ok((_, Err(e)))) => panic!("Elicitation failed: {}", e),
        Ok(Err(e)) => panic!("Server task failed: {:?}", e),
        Err(_) => panic!("Test timed out"),
    }
}
