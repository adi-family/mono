/** Keep existing ADI chat bookmarks usable when the React UI becomes the primary panel. */
export function legacyChatDestination(pathname: string): string | null {
  const parts = pathname.replace(/\/+$/, "").split("/");
  if (parts[0] !== "" || parts[1] !== "chat") return null;
  try {
    const segments = parts.slice(2).map(decodeURIComponent);
    let node: string | undefined;
    if (segments[0]?.startsWith("@")) {
      node = segments.shift()!.slice(1);
      if (!/^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(node)) return null;
    }
    if (segments.length > 2 || segments.some((segment) => !segment)) return null;
    // An explicit empty run keeps unselected and agent-only bookmarks from resuming history.
    // It also makes /chat select this machine even when only remote sources were saved.
    return `/interface?${new URLSearchParams({
      ...(segments[0] ? { agent: segments[0] } : {}),
      run: segments[1] ?? "",
      ...(node ? { node } : {}),
    })}`;
  } catch { return null; }
}
