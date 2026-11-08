import { useChatList } from "../lib/app-chat-list";

export function ChatMutationNotice() {
  const { mutationError, dismissMutationError } = useChatList();
  if (!mutationError) return null;
  return (
    <div role="alert" data-slot="chat-mutation-error" className="bg-popover text-popover-foreground border-border fixed right-4 bottom-4 z-50 flex max-w-sm items-start gap-3 rounded-md border p-3 text-sm shadow-md">
      <p>{mutationError}</p>
      <button type="button" aria-label="Dismiss chat error" className="shrink-0 underline" onClick={dismissMutationError}>Dismiss</button>
    </div>
  );
}
