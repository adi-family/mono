import * as React from "react";
import { ChevronDown } from "lucide-react";

import { cn } from "../lib/cn";

type ScrollSidebarLink = {
  /** The ID of a section on the page. List sections in document order. */
  id: string;
  label: string;
};

type ScrollSidebarGroup = {
  label: string;
  links: readonly ScrollSidebarLink[];
};

type ScrollSidebarLinkProps = React.ComponentProps<"a"> & { href: string };

type ScrollSidebarProps = Omit<React.ComponentProps<"aside">, "children"> & {
  groups: readonly ScrollSidebarGroup[];
  title?: string;
  browseLabel?: string;
  navigationLabel?: string;
  /** Disable tracking when the sections belong to another page. Links remain usable. */
  enabled?: boolean;
  /** Optional page path before the fragment, for example "/library". */
  hrefPrefix?: string;
  /** Change this after client-side navigation so newly rendered sections are restored. */
  navigationKey?: string | number;
  /** Render a router link, forwarding all supplied anchor props. */
  renderLink?: (props: ScrollSidebarLinkProps, link: ScrollSidebarLink) => React.ReactNode;
};

function renderAnchor(props: ScrollSidebarLinkProps) {
  return <a {...props} />;
}

function ScrollSidebar({
  groups,
  title = "On this page",
  browseLabel = "Browse sections",
  navigationLabel = "Section navigation",
  enabled = true,
  hrefPrefix = "",
  navigationKey,
  renderLink = renderAnchor,
  className,
  onKeyDown,
  ...props
}: ScrollSidebarProps) {
  const links = React.useMemo(() => groups.flatMap((group) => group.links), [groups]);
  const navigationId = React.useId();
  const [expanded, setExpanded] = React.useState(false);
  const [activeId, setActiveId] = React.useState<string | undefined>(links[0]?.id);
  const toggleRef = React.useRef<HTMLButtonElement>(null);
  const navigationFrame = React.useRef(0);
  const destination = React.useRef<{ id: string; scrollY: number } | null>(null);
  const pendingFocus = React.useRef<string | null>(null);
  const lastNavigation = React.useRef<string | null>(null);

  const navigateTo = React.useCallback((id: string, focus = false) => {
    const target = document.getElementById(id);
    if (!target) return;
    target.scrollIntoView({ behavior: "instant" });
    destination.current = { id, scrollY: window.scrollY };
    lastNavigation.current = window.location.href;
    setActiveId(id);
    if (focus) target.focus({ preventScroll: true });
  }, []);

  React.useEffect(() => {
    if (!enabled) {
      destination.current = null;
      lastNavigation.current = null;
      return;
    }
    let frame = 0;
    const updateActive = () => {
      // Short sections near the bottom cannot reach the top of the viewport.
      // Preserve an explicit destination until the reader actually scrolls.
      if (destination.current && Math.abs(window.scrollY - destination.current.scrollY) < 1) {
        setActiveId(destination.current.id);
        return;
      }
      destination.current = null;
      const sections = links.flatMap(({ id }) => {
        const element = document.getElementById(id);
        return element ? [element] : [];
      });
      const current = sections.filter((section) => {
        const marker = (Number.parseFloat(getComputedStyle(section).scrollMarginTop) || 0) + 1;
        return section.getBoundingClientRect().top <= marker;
      }).at(-1);
      const atBottom = window.scrollY > 0 && window.scrollY + window.innerHeight >= document.documentElement.scrollHeight - 2;
      setActiveId((atBottom ? sections.at(-1) : current)?.id ?? sections[0]?.id);
    };
    const scheduleUpdate = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(updateActive);
    };
    const restoreHash = () => {
      let id = "";
      try { id = decodeURIComponent(window.location.hash.slice(1)); } catch { /* Ignore malformed fragments. */ }
      // A router's passive effect can arrive after the click already navigated
      // and the reader started scrolling. Do not jump to that fragment twice.
      if (lastNavigation.current !== window.location.href) {
        destination.current = null;
        if (links.some((link) => link.id === id)) navigateTo(id, pendingFocus.current === id);
      }
      pendingFocus.current = null;
      scheduleUpdate();
    };
    const resize = () => {
      destination.current = null;
      scheduleUpdate();
    };
    // Routed sections render after the browser first reads the URL fragment.
    const restoreFrame = requestAnimationFrame(restoreHash);
    window.addEventListener("hashchange", restoreHash);
    window.addEventListener("scroll", scheduleUpdate, { passive: true });
    window.addEventListener("resize", resize);
    return () => {
      cancelAnimationFrame(restoreFrame);
      cancelAnimationFrame(frame);
      cancelAnimationFrame(navigationFrame.current);
      window.removeEventListener("hashchange", restoreHash);
      window.removeEventListener("scroll", scheduleUpdate);
      window.removeEventListener("resize", resize);
    };
  }, [enabled, links, navigationKey, navigateTo]);

  return (
    <aside
      data-slot="scroll-sidebar"
      className={cn("min-w-0", className)}
      onKeyDown={(event) => {
        onKeyDown?.(event);
        if (!event.defaultPrevented && event.key === "Escape" && expanded) {
          setExpanded(false);
          toggleRef.current?.focus();
        }
      }}
      {...props}
    >
      <p className="mb-3 hidden px-2 py-1 text-sm font-semibold md:block">{title}</p>
      <div>
        <button
          ref={toggleRef}
          data-slot="scroll-sidebar-toggle"
          type="button"
          aria-expanded={expanded}
          aria-controls={navigationId}
          className="focus-visible:ring-ring flex w-full items-center gap-3 rounded px-1 py-1 text-left text-sm outline-none focus-visible:ring-2 md:hidden"
          onClick={() => setExpanded((current) => !current)}
        >
          <span className="font-medium">{browseLabel}</span>
          <span className="text-muted-foreground min-w-0 flex-1 truncate text-xs">
            {enabled ? links.find((link) => link.id === activeId)?.label : ""}
          </span>
          <ChevronDown aria-hidden="true" className={cn("size-4 shrink-0", expanded && "rotate-180")} />
        </button>
        <nav
          id={navigationId}
          aria-label={navigationLabel}
          className={cn("mt-3 max-h-[calc(100svh-10rem)] space-y-5 overflow-y-auto md:mt-0 md:block md:max-h-[calc(100svh-16rem)]", !expanded && "hidden")}
        >
          {groups.map((group) => (
            <div key={group.label}>
              <p className="text-muted-foreground mb-2 px-2 text-xs font-medium">{group.label}</p>
              <ul className="grid grid-cols-2 gap-1 md:grid-cols-1">
                {group.links.map((link) => (
                  <li key={link.id}>
                    {renderLink({
                      href: `${hrefPrefix}#${encodeURIComponent(link.id)}`,
                      "aria-current": enabled && activeId === link.id ? "location" : undefined,
                      className: cn(
                        "focus-visible:ring-ring block rounded-md px-2 py-1.5 text-sm outline-none transition-colors focus-visible:ring-2",
                        enabled && activeId === link.id ? "bg-accent text-accent-foreground font-medium" : "text-muted-foreground hover:bg-accent/60 hover:text-foreground",
                      ),
                      onClick: (event) => {
                        if (event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
                        lastNavigation.current = null;
                        setExpanded(false);
                        setActiveId(link.id);
                        pendingFocus.current = link.id;
                        cancelAnimationFrame(navigationFrame.current);
                        // Collapse the mobile menu before measuring the destination.
                        navigationFrame.current = requestAnimationFrame(() => {
                          navigateTo(link.id, true);
                        });
                      },
                      children: link.label,
                    }, link)}
                  </li>
                ))}
              </ul>
            </div>
          ))}
        </nav>
      </div>
    </aside>
  );
}

export {
  ScrollSidebar,
  type ScrollSidebarProps,
  type ScrollSidebarGroup,
  type ScrollSidebarLink,
  type ScrollSidebarLinkProps,
};
