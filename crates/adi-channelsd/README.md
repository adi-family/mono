# adi-channelsd

The local Telegram/Slack client, supervised by Hive independently of the control panel.
The public Cloudflare router remains in `apps/channel-router`.

Hive starts the daemon continuously, reserves the first available managed HTTP port, and
injects it as `PORT`. The daemon binds that port on loopback; it refuses to start without
a nonzero `PORT`. Its API is available at `http://channels.adi` (or
`channels.<ADI_DOMAIN>` for another installation flavor).

`adi-core` provisions two definitions in the installation's store:

- `channels/hive.yaml`: the `channels/client` service, imported by the user Hive supervisor.
- `channels/routes.yaml`: the same service's hostname without a runner, imported by the
  front door. Both definitions resolve to the same port-manager lease.

The definitions omit the listening port. Hive allocates it and retains the lease across
restarts. Definitions are refreshed by bootstrap and app startup; Hive hot-reloads them.
No DNS or front-door restart is needed.

The daemon owns `/api/channels`, `/api/channels/status`, and the existing connect, disconnect,
route, pause, allow, and reply endpoints. `adi-app` forwards its channel API to the internal
domain. Local callers reach the existing user Hive listener and select `channels.<domain>`
with the HTTP Host header, so they also work on Linux installations without local DNS.
The CLI and `channel-reply` tool keep working with the control panel stopped.
`/api/health` identifies the service as `adi-channelsd`.

The service restores one router WebSocket per provider from saved connections and tokens.
Agent answers are posted by the existing turn watchers. Pending questions are polled from
bound conversations with delivery checkpoints; the daemon does not drain the shared event
spool or compete with the app's trigger dispatcher.

## Router configuration

The default external router is `https://hooks.withadi.dev`. `ADI_CHANNEL_ROUTER_URL` overrides
it for another deployment or a separately managed development router. The optional
`ADI_CHANNEL_ROUTER_ADMIN_SECRET` retains its operator-registration behavior. Provisioning
preserves these settings in the daemon's private Hive definition.

`ADI_CHANNELS_BIN` selects the daemon executable when provisioning a development build;
otherwise the executable is resolved beside the other installed ADI binaries.

## Build and verify

```sh
cargo build --release -p adi-channelsd -p adi-app
cargo test -p adi-channelsd -p adi-channels
curl http://channels.adi/api/health
curl http://app.adi/api/channels/status
```

Start, stop, and restart the service through Hive. Do not start an extra daemon by hand
against the live installation: one process must own each provider's subscription.
