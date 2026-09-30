# niello-host JSON-RPC methods

Contract between the Niello shell (client) and `niello-host` (server), per
PLAN.md D2. Transport: JSON-RPC 2.0 over stdio. Field names are camelCase.
Rust mirror: `protocol/rust/src/host.rs` (`niello-protocol` crate).

## `initialize` (request)

Params:

```json
{ "clientName": "string", "clientVersion": "string", "solutionPath": "string (optional)" }
```

Result:

```json
{ "hostName": "niello-host", "hostVersion": "string", "capabilities": {} }
```

`hostName` is always the literal `"niello-host"`. `capabilities` is an object;
its keys are not yet specified.

## `niello/ping` (request)

Params: none.

Result:

```json
{ "pong": true, "timestamp": "ISO-8601 string" }
```

## `niello/host/info` (request)

Params: none.

Result:

```json
{
  "dotnetSdks": [{ "version": "string", "path": "string" }],
  "runtime": "string",
  "os": "string"
}
```

## `shutdown` (request)

Params: none. Result: `null`. The host stops accepting work but stays alive
until `exit`.

## `exit` (notification)

Params: none. No response. The host process exits.
