import { useState } from "react";

/*
 * How the chat reports a failure:
 * - Actions a form awaits (sending a message, answering a question) throw, so the form
 *   keeps its input and shows the message next to it.
 * - Failures tied to the open conversation (stopping, attachments) are scoped errors,
 *   shown only while that conversation is selected.
 * - Edits can target any row in the sidebar, so they report through the global notice.
 */

export const errorText = (error: unknown) => error instanceof Error ? error.message : "The app could not complete this request.";

/** Error messages that each belong to one conversation selection. */
export function useScopedErrors() {
  const [errors, setErrors] = useState<Record<string, string>>({});
  return {
    message: (scope: string) => errors[scope] ?? null,
    report: (scope: string, message: string) => setErrors((current) => ({ ...current, [scope]: message })),
    dismiss: (scope: string) => setErrors((current) => {
      if (!(scope in current)) return current;
      const { [scope]: _dismissed, ...remaining } = current;
      return remaining;
    }),
    clear: () => setErrors((current) => Object.keys(current).length ? {} : current),
  };
}
