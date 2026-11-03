import { useEffect, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { LoaderCircle, Mic, Square } from "lucide-react";
import { Button } from "@design-system/ui";
import { getVoice, transcribeAudio } from "../lib/app-chat";

type Recognition = {
  continuous: boolean; interimResults: boolean; lang: string;
  onresult: ((event: { resultIndex: number; results: { length: number; [index: number]: { isFinal: boolean; [index: number]: { transcript: string } } } }) => void) | null;
  onerror: ((event: { error: string }) => void) | null;
  onend: (() => void) | null;
  start: () => void; stop: () => void; abort: () => void;
};
type SpeechWindow = Window & { SpeechRecognition?: new () => Recognition; webkitSpeechRecognition?: new () => Recognition };

/** The existing app's dictation engines: browser recognition or record-and-transcribe. */
export function ChatMicrophone({ onText, disabledReason = "", onBusyChange }: {
  onText: (text: string) => void; disabledReason?: string; onBusyChange: (busy: boolean) => void;
}) {
  const voice = useQuery({ queryKey: ["app-chat", "voice"], queryFn: getVoice, staleTime: 60_000, retry: false });
  const [engine] = useState(() => { try { return localStorage.getItem("adi.voice.engine.v1") || ""; } catch { return ""; } });
  const [state, setState] = useState<"idle" | "opening" | "recording" | "transcribing">("idle");
  const [error, setError] = useState("");
  const live = useRef<{ recognition?: Recognition; recorder?: MediaRecorder; stream?: MediaStream; controller?: AbortController; cancelled: boolean }>({ cancelled: false });
  const currentOnText = useRef(onText);
  currentOnText.current = onText;
  const busy = state !== "idle";
  useEffect(() => { onBusyChange(busy); }, [busy, onBusyChange]);
  useEffect(() => () => {
    const session = live.current;
    session.cancelled = true;
    session.recognition?.abort();
    if (session.recorder?.state === "recording") session.recorder.stop();
    session.stream?.getTracks().forEach((track) => track.stop());
    session.controller?.abort();
    onBusyChange(false);
  }, [onBusyChange]);

  const engines = voice.data?.engines ?? [];
  const chosen = engines.find((item) => item.id === engine && item.ready)
    ?? engines.find((item) => item.id === voice.data?.default_engine && item.ready)
    ?? engines.find((item) => item.ready);
  const Speech = (window as SpeechWindow).SpeechRecognition ?? (window as SpeechWindow).webkitSpeechRecognition;
  const unavailable = disabledReason || (!window.isSecureContext ? "Microphone blocked." : "")
    || (voice.isPending ? "Loading dictation engines." : voice.isError ? "Dictation engines could not be loaded. Refresh the chat to try again." : !chosen ? "No dictation engine is available." : "")
    || (chosen?.in_browser ? !Speech ? "This browser does not support speech recognition." : "" : !navigator.mediaDevices?.getUserMedia || !window.MediaRecorder ? "This browser does not support audio recording." : "");

  async function start() {
    if (busy || unavailable || !chosen) return;
    const session: typeof live.current = { cancelled: false };
    live.current = session;
    setError(""); setState("opening");
    try {
      if (chosen.in_browser) {
        const recognition = new Speech!();
        session.recognition = recognition;
        recognition.continuous = true;
        recognition.interimResults = false;
        recognition.lang = navigator.language;
        recognition.onresult = (event) => {
          if (session.cancelled) return;
          const words = [];
          for (let index = event.resultIndex; index < event.results.length; index++) {
            if (event.results[index].isFinal) words.push(event.results[index][0].transcript);
          }
          if (words.length) currentOnText.current(words.join(" "));
        };
        recognition.onerror = (event) => {
          if (!session.cancelled) { setError(event.error === "not-allowed" ? "Allow microphone access in your browser’s site settings." : `Dictation stopped: ${event.error}.`); setState("idle"); }
        };
        recognition.onend = () => { if (!session.cancelled) setState("idle"); };
        recognition.start(); setState("recording");
        return;
      }
      const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
      session.stream = stream;
      if (session.cancelled) { stream.getTracks().forEach((track) => track.stop()); return; }
      const mimeType = ["audio/webm;codecs=opus", "audio/mp4", "audio/ogg;codecs=opus"].find((mime) => MediaRecorder.isTypeSupported(mime));
      const recorder = new MediaRecorder(stream, mimeType ? { mimeType } : undefined);
      session.recorder = recorder;
      const chunks: Blob[] = [];
      let bytes = 0;
      let tooLarge = false;
      recorder.ondataavailable = (event) => {
        if (event.data.size) { chunks.push(event.data); bytes += event.data.size; }
        if (bytes > 8 * 1024 * 1024 && recorder.state === "recording") { tooLarge = true; recorder.stop(); }
      };
      recorder.onerror = () => {
        stream.getTracks().forEach((track) => track.stop());
        session.cancelled = true;
        setError("Audio recording failed. Please try again."); setState("idle");
      };
      recorder.onstop = () => {
        stream.getTracks().forEach((track) => track.stop());
        if (session.cancelled) return;
        const audio = new Blob(chunks, { type: recorder.mimeType || mimeType || "audio/webm" });
        if (tooLarge || audio.size > 8 * 1024 * 1024 || !audio.size) {
          setError(audio.size ? "Recording is too large. Try a shorter clip (up to 8 MB)." : "No audio was recorded. Try again."); setState("idle"); return;
        }
        setState("transcribing");
        session.controller = new AbortController();
        void transcribeAudio(audio, chosen.id, session.controller.signal).then((text) => {
          if (!session.cancelled) {
            if (text.trim()) currentOnText.current(text);
            else setError("No speech was recognized. Try again.");
          }
        }).catch((cause) => {
          if (!session.cancelled) setError(cause instanceof Error ? cause.message : "Transcription failed. Try again.");
        }).finally(() => { if (!session.cancelled) setState("idle"); });
      };
      recorder.start(1000); setState("recording");
    } catch (cause) {
      session.stream?.getTracks().forEach((track) => track.stop());
      if (!session.cancelled) {
        setError(cause instanceof DOMException && cause.name === "NotAllowedError" ? "Allow microphone access in your browser’s site settings." : "The microphone could not be opened. Please try again.");
        setState("idle");
      }
    }
  }
  function stop() {
    live.current.recognition?.stop();
    if (live.current.recorder?.state === "recording") live.current.recorder.stop();
  }
  return <div data-slot="chat-microphone" className="flex flex-wrap items-center gap-1">
    <Button type="button" variant="ghost" size="icon" className={state === "recording" ? "size-7 text-red-600 dark:text-red-400" : "size-7"}
      aria-label={state === "recording" ? "Stop dictation" : "Dictate message"} aria-pressed={state === "recording"}
      disabled={state === "opening" || state === "transcribing" || (state === "idle" && !!unavailable)}
      disabledReason={state === "opening" ? "Waiting for microphone access." : state === "transcribing" ? "Transcribing your recording." : unavailable || "Dictation is unavailable."}
      onClick={() => state === "recording" ? stop() : void start()}>
      {state === "opening" || state === "transcribing" ? <LoaderCircle aria-hidden="true" className="size-3.5 motion-safe:animate-spin" /> : state === "recording" ? <Square aria-hidden="true" className="size-3 fill-current" /> : <Mic aria-hidden="true" className="size-4" />}
    </Button>
    {busy && <span role="status" className="text-muted-foreground text-[11px]">{state === "recording" ? "Listening…" : state === "transcribing" ? "Transcribing…" : "Opening microphone…"}</span>}
    {error && <span role="alert" className="text-destructive basis-full text-xs">{error}</span>}
  </div>;
}
