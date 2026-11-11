import { createContext, useContext, useEffect, useRef, useState, type ReactNode } from "react";
import { useNavigate } from "react-router-dom";

import {
  attachmentUrl, getConversation, replyToConversation, startConversation, uploadImage, type AgentPeek, type ConversationRef, type ReplyOptions,
} from "./app-chat";
import { errorText, useScopedErrors } from "./app-chat-errors";
import { useChatList } from "./app-chat-list";
import { confirmsReply, latestTurnSeq, selectionKeyOf } from "./app-chat-model";
import { invalidSourceMessage } from "./app-chat-queries";
import { useChatSelection } from "./app-chat-selection";
import { useChatThread } from "./app-chat-thread";
import { conversationHref } from "./app-chat-transcript";

type ImageUpload =
  | { status: "uploading" }
  | { status: "ready"; attachmentId: string; mediaType: string; url: string }
  | { status: "failed"; error: string };
export type DraftImage = { id: string; name: string } & ImageUpload;

const maxImagesPerMessage = 6;
const tooManyImagesMessage = "Attach up to six files per message.";
const mobileViewport = "(max-width: 767px)";

/**
 * The write can succeed before snapshot generation or delivery fails.
 * Reconcile with a read; never repeat the mutation automatically.
 */
async function deliverReply(target: ConversationRef, message: string, options: Required<Pick<ReplyOptions, "attachments" | "mode">>, baselineSeq: number | null): Promise<AgentPeek> {
  try {
    return await replyToConversation(target, message, options);
  } catch (sendError) {
    if (baselineSeq === null) throw sendError;
    const confirmed = await getConversation(target).catch(() => undefined);
    if (!confirmed || !confirmsReply(confirmed, baselineSeq, message, options.attachments)) throw sendError;
    return confirmed;
  }
}

function useComposerState() {
  const { enabled, invalidSource, active, agent, activeSource, selectionKey, routeNamesConversation, isCurrentSelection, openConversation, recordLaunch } = useChatSelection();
  const { snapshot, adoptResponse, refresh } = useChatThread();
  const { conversations } = useChatList();
  const navigate = useNavigate();
  const [drafts, setDrafts] = useState<Record<string, string>>({});
  const [imageDrafts, setImageDrafts] = useState<Record<string, DraftImage[]>>({});
  const imageErrors = useScopedErrors();
  // Uploads settle outside React's update cycle and need the newest list synchronously.
  const imageDraftsRef = useRef(imageDrafts);
  const nextImageId = useRef(0);
  const sending = useRef(new Set<string>());

  function updateImages(key: string, update: (images: DraftImage[]) => DraftImage[]) {
    const next = { ...imageDraftsRef.current, [key]: update(imageDraftsRef.current[key] ?? []) };
    imageDraftsRef.current = next;
    setImageDrafts(next);
  }

  function settleImage(key: string, id: string, upload: ImageUpload) {
    updateImages(key, (images) => images.map((image) => image.id === id ? { id, name: image.name, ...upload } : image));
  }

  function addImages(files: File[]) {
    const key = selectionKey;
    const source = activeSource;
    const room = Math.max(0, maxImagesPerMessage - (imageDraftsRef.current[key]?.length ?? 0));
    if (files.length > room) imageErrors.report(key, tooManyImagesMessage);
    else imageErrors.dismiss(key);
    for (const file of files.slice(0, room)) {
      const id = `draft-image-${++nextImageId.current}`;
      updateImages(key, (images) => [...images, { id, name: file.name, status: "uploading" }]);
      void uploadImage(file, source).then(
        (image) => settleImage(key, id, { status: "ready", attachmentId: image.id, mediaType: image.media_type, url: attachmentUrl(image.id, source) }),
        (error: unknown) => settleImage(key, id, { status: "failed", error: errorText(error) }),
      );
    }
  }

  async function sendMessage(message: string, mode: "regular" | "asap" = "regular") {
    const key = selectionKey;
    const images = imageDraftsRef.current[key] ?? [];
    const attachments = images.flatMap((image) => image.status === "ready" ? [image.attachmentId] : []);
    if (invalidSource) throw new Error(invalidSourceMessage);
    if (!agent) throw new Error("Choose an agent before sending a message.");
    if (sending.current.has(key)) throw new Error("A message is already being sent to this conversation.");
    if (attachments.length !== images.length) throw new Error("Wait for files to upload, or remove failed attachments.");
    if (!message.trim() && !images.length) throw new Error("Write a message or attach a file.");

    const submittedDraft = drafts[key] ?? "";
    sending.current.add(key);
    try {
      if (active) {
        adoptResponse(await deliverReply(active, message, { attachments, mode }, latestTurnSeq(snapshot)));
        setDrafts((current) => current[key] === submittedDraft ? { ...current, [key]: "" } : current);
      } else {
        const created = await startConversation(agent, message, { source: activeSource, attachments });
        recordLaunch({ agent: created.agent, ...(created.source ? { source: created.source } : {}) });
        const createdKey = selectionKeyOf(created, created.agent, created.source);
        setDrafts((current) => ({ ...current, [key]: "", [createdKey]: current[key] === submittedDraft ? "" : current[key] ?? "" }));
        if (isCurrentSelection(key)) openConversation(created);
      }
      updateImages(key, (current) => current.filter((image) => !images.some((submitted) => submitted.id === image.id)));
      void refresh();
    } catch (error) {
      setDrafts((current) => current[key] ? current : { ...current, [key]: submittedDraft });
      throw error;
    } finally {
      sending.current.delete(key);
    }
  }

  const recent = conversations.find((conversation) => conversation.source === activeSource);
  const recentHref = recent ? conversationHref(recent) : null;
  const isComposing = Boolean(drafts[selectionKey]) || Boolean(imageDrafts[selectionKey]?.length);
  // On desktop an unaddressed chat page opens the newest conversation, unless one is being written.
  useEffect(() => {
    if (!enabled || routeNamesConversation || !recentHref || isComposing || sending.current.has(selectionKey)) return;
    if (!window.matchMedia(mobileViewport).matches) navigate(recentHref, { replace: true });
  }, [enabled, routeNamesConversation, recentHref, isComposing, selectionKey, navigate]);

  return {
    draft: drafts[selectionKey] ?? "",
    setDraft: (value: string) => setDrafts((current) => ({ ...current, [selectionKey]: value })),
    appendDraft: (text: string) => setDrafts((current) => ({ ...current, [selectionKey]: [current[selectionKey]?.trimEnd(), text.trim()].filter(Boolean).join(" ") })),
    images: imageDrafts[selectionKey] ?? [],
    imageError: imageErrors.message(selectionKey),
    addImages,
    removeImage: (id: string) => updateImages(selectionKey, (images) => images.filter((image) => image.id !== id)),
    sendMessage,
  };
}

const ChatComposerContext = createContext<ReturnType<typeof useComposerState> | null>(null);

export function ChatComposerProvider({ children }: { children: ReactNode }) {
  return <ChatComposerContext.Provider value={useComposerState()}>{children}</ChatComposerContext.Provider>;
}

/** The message being written. Only this context changes on a keystroke. */
export function useChatComposer() {
  const value = useContext(ChatComposerContext);
  if (!value) throw new Error("useChatComposer must be inside AppChatProvider.");
  return value;
}
