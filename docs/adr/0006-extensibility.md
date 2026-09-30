# ADR-0006: WASM extensions

Status: Accepted, 2026-10-01
Plan reference: PLAN.md section 3 (D6), section 7

## Context

Niello is not a VS Code extension host (PLAN.md section 1). Third parties still need to add themes, grammars, small commands and language or debugger registrations. Adding a language costs a tree-sitter grammar, an LSP registration, a DAP registration and test-runner glue, all of which can be declared rather than coded (PLAN.md section 7).

Extensions run inside a product that must never freeze (ADR-0001 and ADR-0002) and that exposes powerful commands to agents. An in-process native plugin could crash or stall the shell, and gives unsandboxed access to the user's machine.

The extension surface is a contract that outsiders link against, so it must be permissively licensed (ADR-0005).

## Decision

- Version 1 extensions are WASM components run by wasmtime in the `extensions` crate.
- Each extension ships a capability manifest. Capabilities are granted explicitly and the sandbox denies everything else.
- The extension API lives in `extension-sdk/` (crate `niello-extension-sdk`), licensed MIT.
- In v1 an extension can contribute: themes, grammars, small commands, and language and debugger registrations (LSP, DAP, tree-sitter, test-runner glue).
- Extension commands register on the same command bus as built-ins, with the same schemas and permission classes. Agents see them like any other command.
- Heavyweight integrations move to out-of-process .NET or native tool hosts later, speaking the protocols from ADR-0003.
- No in-process native plugins, ever.
- The Extensions dialog is VS-style and backed by a registry we host. The registry and auto-update arrive in Phase 2.

## Alternatives considered

- VS Code extension compatibility: would pull in a Node runtime and a very large, moving API. Excluded by the non-goals.
- In-process native plugins (dynamic libraries): fast and flexible, but a crash or stall hits the UI thread and there is no sandbox.
- Embedded scripting (Lua, JavaScript): easy to write, but sandboxing and a typed API are weaker than the component model, and it adds a second runtime.
- .NET plugin assemblies loaded into `niello-host`: natural for C# authors, but couples extensions to host internals and risks Roslyn stability.
- No extension system before 1.0: simplest, but themes, grammars and language registrations are the cheapest way to reach the polylingual goal.

## Consequences

Positive:
- Sandboxed by default, with an auditable capability list per extension.
- Any language that compiles to WASM components can write extensions.
- A crashing or hanging extension cannot take down the shell if calls are bounded and cancelable.
- A typed interface suits agent-written extensions and generated bindings.

Negative:
- The component model and wasmtime are still evolving, so the SDK may need breaking changes before it stabilizes.
- A WASM boundary limits what extensions can do. Some integrations need a separate process host, which is more work for authors.
- We own the registry, review process and update flow. That is real operational cost for one maintainer.
- The capability model must be designed carefully: too coarse is unsafe, too fine is unusable.

## Revisit when

- The WASM component model or wasmtime stops being a viable base.
- Several wanted extensions cannot be expressed within the capability model and all need an out-of-process host.
- Registry hosting becomes a network service, which also reopens the AGPL question in ADR-0005.
- Phase 2 or Phase 5 language work shows the declarative registration vocabulary is too small.
