//! `eludite-openai-acp`: an Agent Client Protocol (ACP) agent over any server that speaks the OpenAI Chat
//! Completions API (llama.cpp's `llama-server`, Ollama, vLLM, LM Studio, OpenRouter, Groq, Together, DeepSeek,
//! Mistral, OpenAI), whose only tools are the IDE's, reached through the MCP servers the ACP client passes in
//! `session/new` (Eludite's endpoint, brief 0016). Brief 0060, ADR-0013.
//!
//! ACP side: [`agent`] (protocol version 1 through the official `agent-client-protocol` crate). Model side:
//! [`provider`] (`GET /models`, the streamed `POST /chat/completions`, the SSE and chunk rules) and [`models`]
//! (listing, the catalog, the `model` config option). Tools: [`mcp`] (the MCP client over stdio or HTTP, the
//! mapping to function tools, the core set and `eludite-tools`). The turn: [`turn`] (`src/loop.rs`: request,
//! stream, run tools, repeat), [`compact`] (trimming and summarizing to fit the window) and [`prompt`] (the system
//! prompt). [`log`] keeps the key out of every log line.
//!
//! Public API boundary: the binary's command line (`eludite-openai-acp --base-url URL ...` serves ACP on stdio;
//! `eludite-openai-acp models --base-url URL ...` prints a model listing as JSON), the key in
//! `$ELUDITE_OPENAI_API_KEY`, and ACP on stdio. The library exists for the binary, its tests and its benchmark.

pub mod agent;
pub mod compact;
pub mod log;
pub mod mcp;
pub mod models;
pub mod prompt;
pub mod provider;
#[path = "loop.rs"]
pub mod turn;
