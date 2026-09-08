//! The adi LLM gateway: one loopback endpoint that every model client on this machine can be
//! pointed at, which forwards the request to the real provider and writes down what went past.
//!
//! # Why a base-URL gateway and not an interception proxy
//!
//! Every client that matters here already takes its endpoint from the environment —
//! `ANTHROPIC_BASE_URL` for the `claude` CLI, `OPENAI_BASE_URL` for the `OpenAI` SDKs, `base_url` on
//! a `harness:adi` agent — so pointing them at this process needs no certificate, no DNS override
//! and no root. That matters more here than convenience: the front door's own CA carries X.509
//! name constraints permitting `adi`, `localhost` and `127.0.0.0/8` and nothing else
//! (`adi_hive::tls`), so it *cannot* vouch for `api.anthropic.com` even if somebody wanted it to.
//! Interception would need a second, unconstrained root in the system trust store — which is the
//! one thing this platform has deliberately refused to install.
//!
//! # The shape
//!
//! The first path segment names the provider and the rest is handed on untouched:
//! `POST /anthropic/v1/messages` becomes `https://api.anthropic.com/v1/messages`. So a client only
//! ever learns one new thing, its base URL, and goes on building every path below it itself.
//!
//! What the gateway does *not* do is as deliberate as what it does: it adds no key and answers from
//! no cache. Authorization is the client's own header, forwarded as sent. The one header it drops
//! on the way out is `accept-encoding`, so the provider answers in plain text and the journal holds
//! something a human can read.
//!
//! The single exception is [`placeholders`], which rewrites a body on its way past to spend fewer
//! tokens on the literals a prompt repeats. It is **experimental and off**, in a fresh config and
//! in this store's own, and it does nothing until an operator enables it *and* a request asks by
//! header. Treat it as a sharp tool rather than an optimisation: it shows the model text its author
//! never wrote, it loses to the prompt cache on any prompt the provider is caching, and an
//! unexpanded placeholder is a plausible-looking wrong path in an answer. Its own module
//! documentation carries the measurements and the full list of ways it can hurt; read that before
//! switching it on.

pub mod config;
pub mod http;
pub mod journal;
pub mod placeholders;
pub mod proxy;
