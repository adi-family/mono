import type { ReactNode } from "react";

import { ChatComposerProvider } from "./app-chat-composer";
import { ChatListProvider } from "./app-chat-list";
import { ChatSelectionProvider } from "./app-chat-selection";
import { ChatThreadProvider } from "./app-chat-thread";

/**
 * Each layer reads the ones above it. Ordering them from slowest to fastest changing means
 * a keystroke re-renders only composer consumers, and a transcript poll leaves selection
 * consumers such as the topbar alone.
 */
export function AppChatProvider({ children }: { children: ReactNode }) {
  return (
    <ChatSelectionProvider>
      <ChatThreadProvider>
        <ChatListProvider>
          <ChatComposerProvider>{children}</ChatComposerProvider>
        </ChatListProvider>
      </ChatThreadProvider>
    </ChatSelectionProvider>
  );
}
