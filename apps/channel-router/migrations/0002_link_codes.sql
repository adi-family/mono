-- Telegram caps a deep link's `start` parameter at 64 characters of [A-Za-z0-9_-], and a signed
-- link code (base64url JSON, ".", signature) is ~200 and carries a dot -- Telegram drops it and
-- the link never completes. So what an operator is handed is a short random handle, and the
-- signed code it stands for waits here until it is used once or expires (ADI-MONO-124).
CREATE TABLE IF NOT EXISTS link_codes (
  handle TEXT PRIMARY KEY,
  signed TEXT NOT NULL,
  expires_at INTEGER NOT NULL
);
