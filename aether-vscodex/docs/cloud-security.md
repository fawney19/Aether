# Cloud security model

## Trust boundaries

- Aether authenticates browser HTTP requests and resolves the user ID. The client never supplies a trusted user ID.
- The Node sidecar never receives an Aether access token or JWT signing key.
- A VS Code installation receives one revocable device credential. Only its scrypt hash is persisted.
- An iframe receives a random, one-time WebSocket ticket with a 60-second lifetime. Tickets are sent in an auth frame, never in a URL.
- The embedded UI is trusted, same-origin Aether code. `allow-same-origin` is required by the current integration, so the iframe is not a sandbox boundary for untrusted content even though the parent does not post its JWT into the frame.
- Relay state is isolated by `(user_id, device_id)`. A browser ticket and host credential must resolve to the same room.

## Network boundary

Run the sidecar on the private Compose network. Do not publish port 8788. Aether gateway is the only public HTTP and WebSocket entry point and authenticates internal API calls with `AETHER_VSCODEX_INTERNAL_TOKEN`.

`AETHER_VSCODEX_ALLOWED_ORIGINS` must contain the exact public Aether origin when the sidecar binds outside loopback. Public deployments must use HTTPS/WSS.

## Current scaling limit

The first release intentionally runs one sidecar replica. Pairing codes, browser tickets, and the live connection directory are process-local. Before adding replicas, move those records to a shared atomic store and add sticky or distributed WebSocket room routing.
