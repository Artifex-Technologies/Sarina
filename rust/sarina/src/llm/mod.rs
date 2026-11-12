//! Go: `go/llm`: the message types (types.go), the repair of damaged JSON (repair.go) and the normaliser for local
//! servers' quirks (normalize.go), the OpenAI-compatible wire format and transport (client.go: `openai.rs`, `client.rs`), the Anthropic client
//! (`anthropic.rs`) and the context-window discovery (`discover.rs`).

pub mod anthropic;
pub mod client;
pub mod discover;
pub mod normalize;
pub mod openai;
pub mod repair;
pub mod types;

/// The model transport (Go's `agent.Client` and `tiers.Completer`): `Client`, `AnthropicClient` and the tier router.
pub trait Completer {
    fn complete(
        &self,
        ctx: &crate::goctx::Ctx,
        req: &openai::Request,
        on: Option<&mut dyn FnMut(openai::StreamEvent)>,
    ) -> Result<types::Turn, client::Error>;
}

impl Completer for client::Client {
    fn complete(
        &self,
        ctx: &crate::goctx::Ctx,
        req: &openai::Request,
        on: Option<&mut dyn FnMut(openai::StreamEvent)>,
    ) -> Result<types::Turn, client::Error> {
        client::Client::complete(self, ctx, req, on)
    }
}

impl Completer for anthropic::AnthropicClient {
    fn complete(
        &self,
        ctx: &crate::goctx::Ctx,
        req: &openai::Request,
        on: Option<&mut dyn FnMut(openai::StreamEvent)>,
    ) -> Result<types::Turn, client::Error> {
        anthropic::AnthropicClient::complete(self, ctx, req, on)
    }
}
