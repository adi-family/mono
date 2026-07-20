//! The preset catalog: ready-made trigger definitions a human applies to prefill the form.
//!
//! A [kind](crate::KIND_BACKGROUND) only says *how* a trigger launches. What it *does* —
//! talk to Telegram, poll a URL, react to a push — is code, and a preset is that code written
//! for you: a kind, a runtime, a working code block, and the named settings the block reads
//! from its environment. Applying one is a pure client-side prefill; nothing here is stored
//! beyond the [`TriggerManifest::preset`](crate::TriggerManifest::preset) breadcrumb that lets
//! the editor re-show a trigger's settings later.
//!
//! Every field a preset declares reaches its code block as `ADI_<KEY>`, uppercased — the
//! `chat_id` field below is `$ADI_CHAT_ID` in the script.

use crate::trigger::{KIND_BACKGROUND, KIND_EVENT, KIND_WEBHOOK, RUNTIME_SH, RUNTIME_TS};

/// One named setting a preset's code block reads from its environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresetField {
    /// The extras key — reaches the code block as `ADI_<KEY>`, uppercased.
    pub key: &'static str,
    /// The editor label.
    pub label: &'static str,
    /// One line on what the value is for.
    pub hint: &'static str,
    /// Prefilled when the preset is applied; empty for a value only the user can supply.
    pub default: &'static str,
}

/// A ready-made trigger definition. `code` is written to be runnable as-is once the preset's
/// fields are filled in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Preset {
    /// Stable id, stored on a trigger that was created from this preset.
    pub id: &'static str,
    /// The editor label ("Telegram bot").
    pub label: &'static str,
    /// One line on what applying this gets you.
    pub description: &'static str,
    /// The kind the preset switches the form to.
    pub kind: &'static str,
    /// The runtime the preset's code block is written in.
    pub runtime: &'static str,
    /// The prefilled code block.
    pub code: &'static str,
    /// The settings the code block reads.
    pub fields: &'static [PresetField],
    /// For an [event](crate::KIND_EVENT) preset: the event-name patterns to prefill the
    /// subscription with (e.g. `adi.tasks.*`). Empty for every other kind.
    pub events: &'static [&'static str],
}

/// Every preset the platform ships, in the order the editor offers them.
#[must_use]
pub fn all() -> &'static [Preset] {
    PRESETS
}

/// The preset with this id, if it exists.
#[must_use]
pub fn get(id: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.id == id)
}

/// The fields a trigger's editor should offer: the ones its preset declares. A trigger with no
/// preset (or an id from a build that shipped one we don't have) declares nothing — its extras
/// are still editable as free-form keys.
#[must_use]
pub fn fields_for(preset: Option<&str>) -> &'static [PresetField] {
    preset.and_then(get).map_or(&[], |p| p.fields)
}

const TOKEN_ENV: PresetField = PresetField {
    key: "token_env",
    label: "Bot token env var",
    hint: "env var holding the bot token",
    default: "TELEGRAM_BOT_TOKEN",
};

const CHAT_ID: PresetField = PresetField {
    key: "chat_id",
    label: "Chat id",
    hint: "the chat to talk to",
    default: "",
};

const INTERVAL: PresetField = PresetField {
    key: "interval_secs",
    label: "Interval (seconds)",
    hint: "seconds between passes",
    default: "300",
};

static PRESETS: &[Preset] = &[
    Preset {
        id: "telegram-bot",
        events: &[],
        label: "Telegram bot",
        description: "Long-polls Telegram for messages and replies. Runs until you disable it.",
        kind: KIND_BACKGROUND,
        runtime: RUNTIME_TS,
        fields: &[TOKEN_ENV, CHAT_ID],
        code: r#"// Telegram bot: long-poll getUpdates and reply. Lives until the trigger is disabled.
const tokenVar = process.env.ADI_TOKEN_ENV ?? "TELEGRAM_BOT_TOKEN";
const token = process.env[tokenVar];
if (!token) throw new Error(`no bot token — set $${tokenVar} in the environment`);

const api = `https://api.telegram.org/bot${token}`;
const onlyChat = process.env.ADI_CHAT_ID;
let offset = 0;

for (;;) {
  try {
    const res = await fetch(`${api}/getUpdates?timeout=50&offset=${offset}`);
    const { result = [] } = await res.json();
    for (const update of result) {
      offset = update.update_id + 1;
      const message = update.message;
      if (!message?.text) continue;
      if (onlyChat && String(message.chat.id) !== onlyChat) continue;

      console.log(`[${message.chat.id}] ${message.text}`);
      await fetch(`${api}/sendMessage`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ chat_id: message.chat.id, text: `echo: ${message.text}` }),
      });
    }
  } catch (e) {
    console.error(`poll failed: ${e}`);
    await new Promise((r) => setTimeout(r, 5000));
  }
}
"#,
    },
    Preset {
        id: "telegram-notify",
        events: &[],
        label: "Telegram notification",
        description: "Forwards whatever calls the webhook straight into a Telegram chat.",
        kind: KIND_WEBHOOK,
        runtime: RUNTIME_SH,
        fields: &[TOKEN_ENV, CHAT_ID],
        code: r#"# Forward the webhook payload to a Telegram chat.
token=$(printenv "${ADI_TOKEN_ENV:-TELEGRAM_BOT_TOKEN}")
[ -n "$token" ] || { echo "no bot token in \$${ADI_TOKEN_ENV:-TELEGRAM_BOT_TOKEN}" >&2; exit 1; }

# Telegram caps a message at 4096 characters.
text=$(head -c 3500 "$ADI_PAYLOAD_FILE")

curl -sS "https://api.telegram.org/bot$token/sendMessage" \
  --data-urlencode "chat_id=$ADI_CHAT_ID" \
  --data-urlencode "text=$ADI_TRIGGER fired:
$text"
"#,
    },
    Preset {
        id: "interval",
        events: &[],
        label: "Every N seconds",
        description: "A loop that does the work, sleeps, and repeats — the scheduled-job shape.",
        kind: KIND_BACKGROUND,
        runtime: RUNTIME_SH,
        fields: &[INTERVAL],
        code: r#"# Runs forever: do the work, sleep, repeat.
# The supervisor restarts this with backoff if it ever exits, so a crash is not fatal.
while :; do
  date -u "+%Y-%m-%dT%H:%M:%SZ — tick"

  # …the work goes here…

  sleep "${ADI_INTERVAL_SECS:-300}"
done
"#,
    },
    Preset {
        id: "http-poll",
        events: &[],
        label: "Watch a URL",
        description: "Polls a URL on an interval and logs whenever the response changes.",
        kind: KIND_BACKGROUND,
        runtime: RUNTIME_TS,
        fields: &[
            PresetField {
                key: "url",
                label: "URL",
                hint: "the endpoint to poll",
                default: "",
            },
            INTERVAL,
        ],
        code: r#"// Poll a URL and report whenever its response body changes.
const url = process.env.ADI_URL;
if (!url) throw new Error("no URL — fill in the `url` setting");
const every = Number(process.env.ADI_INTERVAL_SECS ?? 300) * 1000;

let previous: string | undefined;

for (;;) {
  const at = new Date().toISOString();
  try {
    const body = await (await fetch(url)).text();
    if (previous === undefined) {
      console.log(`[${at}] watching ${url} (${body.length} bytes)`);
    } else if (body !== previous) {
      console.log(`[${at}] changed: ${previous.length} → ${body.length} bytes`);

      // …react to the change here…
    }
    previous = body;
  } catch (e) {
    console.error(`[${at}] poll failed: ${e}`);
  }
  await new Promise((r) => setTimeout(r, every));
}
"#,
    },
    Preset {
        id: "webhook-echo",
        events: &[],
        label: "Echo the payload",
        description: "Logs every inbound call — the one to start from when wiring a new webhook.",
        kind: KIND_WEBHOOK,
        runtime: RUNTIME_SH,
        fields: &[],
        code: r#"# Log the inbound call, then do something with it.
echo "$ADI_TRIGGER fired at $(date -u '+%Y-%m-%dT%H:%M:%SZ')"
echo '--- payload ---'
cat "$ADI_PAYLOAD_FILE"
"#,
    },
    Preset {
        id: "github-push",
        events: &[],
        label: "GitHub push",
        description: "Parses a GitHub webhook delivery and reports the commits that landed.",
        kind: KIND_WEBHOOK,
        runtime: RUNTIME_TS,
        fields: &[],
        code: r#"// Handle a GitHub webhook delivery. Point the repo's webhook at this trigger's URL,
// set its secret to match, and pick the events you care about.
const raw = await Bun.file(process.env.ADI_PAYLOAD_FILE!).text();
const payload = JSON.parse(raw);

const branch = payload.ref?.replace("refs/heads/", "");
if (!branch) {
  console.log("not a push — ignored");
  process.exit(0);
}

console.log(`push to ${payload.repository?.full_name}@${branch} by ${payload.pusher?.name}`);
for (const commit of payload.commits ?? []) {
  console.log(`  ${commit.id.slice(0, 7)} ${commit.message.split("\n")[0]}`);
}

// …deploy, notify, whatever this push should cause…
"#,
    },
    Preset {
        id: "event",
        events: &["adi.tasks.*"],
        label: "On a platform event",
        description: "Runs whenever a platform event matches its patterns — e.g. adi.tasks.* on any task change.",
        kind: KIND_EVENT,
        runtime: RUNTIME_TS,
        fields: &[],
        code: r#"// Runs when a subscribed platform event fires (edit the "Events" patterns above).
// $ADI_EVENT is the concrete event name that matched; $ADI_PAYLOAD is its JSON body.
const event = process.env.ADI_EVENT ?? "(unknown)";
const payload = JSON.parse(process.env.ADI_PAYLOAD ?? "{}");

console.log(`event: ${event}`);
console.log(JSON.stringify(payload, null, 2));

// …react to the event here — notify, kick off a build, update something…
"#,
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trigger::{normalize_kind, normalize_runtime};

    #[test]
    fn every_preset_is_coherent() {
        for p in all() {
            assert!(!p.id.is_empty(), "a preset needs an id");
            assert!(!p.code.trim().is_empty(), "{} has no code", p.id);
            assert_eq!(normalize_kind(p.kind), p.kind, "{} has a dead kind", p.id);
            assert_eq!(
                normalize_runtime(p.runtime),
                p.runtime,
                "{} has a dead runtime",
                p.id
            );
        }
    }

    #[test]
    fn preset_ids_are_unique() {
        let mut ids: Vec<_> = all().iter().map(|p| p.id).collect();
        ids.sort_unstable();
        let count = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), count, "preset ids must be unique");
    }

    /// A preset's fields are what the editor shows and what its code block reads, so a code
    /// block referencing `$ADI_FOO` without declaring `foo` would silently never be fillable.
    #[test]
    fn declared_fields_cover_the_env_vars_each_code_block_reads() {
        // Set by the platform for every trigger, so a block may read these without declaring them.
        let platform = [
            "ADI_TRIGGER",
            "ADI_TRIGGER_KIND",
            "ADI_PAYLOAD_FILE",
            "ADI_PAYLOAD",
            "ADI_EVENT",
        ];
        for p in all() {
            let declared: Vec<String> = p
                .fields
                .iter()
                .map(|f| format!("ADI_{}", f.key.to_uppercase()))
                .collect();
            for var in referenced_adi_vars(p.code) {
                assert!(
                    platform.contains(&var.as_str()) || declared.contains(&var),
                    "preset {} reads {var} but declares no matching field",
                    p.id
                );
            }
        }
    }

    /// Every `ADI_…` identifier appearing in a code block, however it is spelled (`$ADI_X`,
    /// `${ADI_X}`, `process.env.ADI_X`).
    fn referenced_adi_vars(code: &str) -> Vec<String> {
        let mut found = Vec::new();
        let bytes: Vec<char> = code.chars().collect();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i..].starts_with(&['A', 'D', 'I', '_']) {
                let end = bytes[i..]
                    .iter()
                    .position(|c| !(c.is_ascii_alphanumeric() || *c == '_'))
                    .map_or(bytes.len(), |off| i + off);
                found.push(bytes[i..end].iter().collect());
                i = end;
            } else {
                i += 1;
            }
        }
        found
    }

    #[test]
    fn lookup_finds_presets_and_their_fields() {
        assert_eq!(get("telegram-bot").map(|p| p.kind), Some(KIND_BACKGROUND));
        assert!(get("no-such-preset").is_none());
        assert_eq!(fields_for(Some("telegram-bot")).len(), 2);
        assert!(fields_for(None).is_empty());
        assert!(fields_for(Some("no-such-preset")).is_empty());
    }

    /// The event preset is the only one that ships subscription patterns, and only event presets
    /// do: `events` is empty for every non-event kind.
    #[test]
    fn only_the_event_preset_carries_subscription_patterns() {
        let event = get("event").expect("the event preset exists");
        assert_eq!(event.kind, KIND_EVENT);
        assert_eq!(event.events, &["adi.tasks.*"]);
        for p in all() {
            assert_eq!(
                p.kind == KIND_EVENT,
                !p.events.is_empty(),
                "{} carries patterns iff it is an event preset",
                p.id
            );
        }
    }
}
