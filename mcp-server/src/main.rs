//! # AgentPay MCP Server
//!
//! A Model Context Protocol server that exposes three tools to an AI
//! agent runtime:
//!
//! - `discover_resources` — find paid resources matching a query
//! - `check_budget` — query remaining spending allowance
//! - `pay_and_call` — pay for and invoke a resource
//!
//! Runs over stdio transport for local agent-runtime use.

mod account_state;
mod policy;
mod soroban_client;
mod tools;

use rmcp::schemars::JsonSchema;
use rmcp::{
    ErrorData as McpError, handler::server::wrapper::Parameters, model::*, tool, tool_handler,
    tool_router,
};
use serde::Deserialize;

// ---------------------------------------------------------------------------
// Tool Argument Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct DiscoverArgs {
    query: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct PayArgs {
    /// Registry resource ID (e.g. "weather-oracle", "price-feed") for legacy catalog calls.
    #[serde(default)]
    resource_id: Option<String>,
    /// JSON parameters for the resource call.
    #[serde(default)]
    params: Option<String>,
    /// Target HTTPS URL for direct x402 HTTP resource payment and invocation.
    #[serde(default)]
    url: Option<String>,
}

// ---------------------------------------------------------------------------
// MCP Server definition
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct AgentPayServer;

#[tool_router]
impl AgentPayServer {
    /// Discover paid resources matching a search query.
    #[tool(
        description = "Search for paid resources (APIs, datasets, on-chain services) available for the agent to call. Returns matching resources with pricing and contract details."
    )]
    async fn discover_resources(
        &self,
        Parameters(args): Parameters<DiscoverArgs>,
    ) -> Result<String, McpError> {
        let results = tools::discover::search_resources(&args.query);
        let text = serde_json::to_string_pretty(&results)
            .unwrap_or_else(|e| format!("{{\"error\": \"Failed to serialize results: {e}\"}}"));
        Ok(text)
    }

    /// Check the agent's remaining spending budget.
    #[tool(
        description = "Check the agent's remaining spending budget on the smart account. Returns remaining allowance in stroops and XLM, the budget period, and number of active rules."
    )]
    async fn check_budget(&self) -> Result<String, McpError> {
        let status = tools::check_budget::check_budget();
        let text = serde_json::to_string_pretty(&status).unwrap_or_else(|e| {
            format!("{{\"error\": \"Failed to serialize budget status: {e}\"}}")
        });
        Ok(text)
    }

    /// Pay for and call a resource.
    #[tool(
        description = "Pay for and invoke a paid resource. Supports either an x402 paid HTTPS URL or a registry resource ID. Submits payment through the agent's smart account or external payer worker, enforcing spending policies. Returns status, body, tx hash, amount, asset, and pay_to. Errors include: InvalidUrl, BlockedTarget, PaymentRequirementsInvalid, NetworkMismatch, OverCeiling, PolicyDenied, BudgetExceeded, PayerFailed, PayerTimeout, FacilitatorRejected, SettlementMissing, InsufficientBudget, ResourceNotFound, ResourceCallFailed."
    )]
    async fn pay_and_call(
        &self,
        Parameters(args): Parameters<PayArgs>,
    ) -> Result<String, McpError> {
        let text = match tools::pay_and_call::pay_and_call_unified(
            args.url.as_deref(),
            args.resource_id.as_deref(),
            args.params.as_deref().unwrap_or("{}"),
        ) {
            Ok(result) => serde_json::to_string_pretty(&result)
                .unwrap_or_else(|e| format!("{{\"error\": \"Failed to serialize result: {e}\"}}")),
            Err(e) => e.to_structured_json(),
        };
        Ok(text)
    }
}

#[tool_handler]
impl rmcp::ServerHandler for AgentPayServer {
    fn get_info(&self) -> InitializeResult {
        InitializeResult::new(ServerCapabilities::builder().enable_tools().build())
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    tracing::info!("AgentPay MCP Server starting on stdio...");

    // Create the server and run on stdio transport
    let server = AgentPayServer;
    use rmcp::ServiceExt;
    let service = server.serve(rmcp::transport::io::stdio()).await?;

    // Wait for the service to complete
    service.waiting().await?;

    Ok(())
}
