import type { AgentChatStats } from "./app-chat";

/** Return an exact error count only when the server's retained references cover this range. */
export function groupErrorCount(
  stats: AgentChatStats | undefined, turn: number, from: number, to: number,
): number | undefined {
  if (!stats || ![turn, from, to].every((value) => Number.isSafeInteger(value) && value >= 0) || to < from) return;
  if (from === to) return 0;
  const total = stats.by_tool.reduce((sum, use) => sum + use.failed, 0);
  if (total > stats.failed.length) {
    // A truncated list holds the earliest failures. Later groups need their expanded details.
    const last = stats.failed.reduce<{ turn: number; step: number } | undefined>((latest, ref) =>
      !latest || ref.turn > latest.turn || (ref.turn === latest.turn && ref.step > latest.step) ? ref : latest,
    undefined);
    if (!last || turn > last.turn || (turn === last.turn && to - 1 > last.step)) return;
  }
  return stats.failed.filter((ref) => ref.turn === turn && ref.step >= from && ref.step < to).length;
}
