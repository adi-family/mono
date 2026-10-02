/**
 * `RegisterLimiter` -- a per-IP sliding-window counter for open (non-admin) `POST /register`
 * calls (ADI-MONO-125, `router.ts`'s `checkRegisterRateLimit`). One Durable Object per client
 * IP (`idFromName(ip)`), so two IPs never contend and nothing is shared across them.
 *
 * A Durable Object, not Workers' native Rate Limiting binding, for the same reason every other
 * time-sensitive check in this router takes an injected clock instead of `Date.now()`: the
 * binding has no way to be given a test-pinned `now`, so a vitest suite could never deterministically
 * exercise "the window just expired" or "one request short of the cap". `router.ts`'s `handle`
 * already threads a test-pinnable `now` through everything else; this is that same clock, one
 * level further in.
 */

/** How many open registrations one IP may make inside {@link WINDOW_SECONDS} -- generous enough
 * that a human retrying a flaky connect attempt (or a CLI script in a test loop) never notices,
 * tight enough that a flood of registrations from one address still gets throttled. */
const MAX_ATTEMPTS = 10;

/** The window {@link MAX_ATTEMPTS} is counted over, in seconds. A fixed window (reset on expiry,
 * not a rolling one) -- simpler, and the few extra requests a request landing right at the reset
 * boundary could buy an abuser is not worth a sliding-log implementation for this. */
const WINDOW_SECONDS = 60;

interface CheckRequest {
  /** Epoch seconds, from `router.ts`'s own (possibly test-pinned) clock. */
  now: number;
}

export class RegisterLimiter implements DurableObject {
  constructor(private readonly ctx: DurableObjectState) {}

  async fetch(request: Request): Promise<Response> {
    const { now } = (await request.json()) as CheckRequest;

    const windowStart = (await this.ctx.storage.get<number>("window_start")) ?? now;
    let count = (await this.ctx.storage.get<number>("count")) ?? 0;
    if (now - windowStart >= WINDOW_SECONDS) {
      await this.ctx.storage.put("window_start", now);
      count = 0;
    }
    count += 1;
    await this.ctx.storage.put("count", count);

    return Response.json({ allowed: count <= MAX_ATTEMPTS });
  }
}
