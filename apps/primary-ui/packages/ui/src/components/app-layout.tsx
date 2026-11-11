import * as React from "react";

import { cn } from "../lib/cn";
import { type LocalStorageCodec, useLocalStorageState } from "../lib/use-local-storage-state";
import { AppContent, AppShell, AppSidebar, AppTopbar } from "./app-shell";

type AppLayoutProps = Omit<React.ComponentProps<"div">, "children"> & {
  primaryNavigation: React.ReactNode;
  secondaryNavigation?: React.ReactNode;
  topbar: React.ReactNode;
  children: React.ReactNode;
  /** Join the second menu and content in one bounded surface beside the primary navigation. */
  surface?: "separate" | "joined";
  /** Opt in to remembering the resized sidebar width in local storage. */
  sidebarWidthStorageKey?: string;
  /** Hide the primary rail and topbar while retaining section navigation. */
  focusMode?: boolean;
  /** Floating controls at the workspace origin, independent of section navigation. */
  workspaceControls?: React.ReactNode;
  surfaceProps?: Omit<React.ComponentProps<"div">, "children">;
  /** Hide the second menu without unmounting its contents. */
  hideSecondaryNavigation?: boolean;
  primaryNavigationProps?: Omit<React.ComponentProps<"aside">, "children">;
  secondaryNavigationProps?: Omit<React.ComponentProps<"div">, "children" | "hidden">;
  topbarProps?: Omit<React.ComponentProps<"header">, "children">;
  contentProps?: Omit<React.ComponentProps<"div">, "children">;
};

const defaultSidebarWidth = 200;

function clampSidebarWidth(value: number) {
  return Math.max(160, Math.min(480, value));
}

// The width is stored as a bare number, as earlier releases wrote it; anything else resets to the default.
const sidebarWidthCodec: LocalStorageCodec<number | undefined> = {
  parse: (stored) => {
    const value = stored.trim() ? Number(stored) : NaN;
    return Number.isFinite(value) ? clampSidebarWidth(value) : undefined;
  },
  serialize: (value) => (value === undefined ? undefined : String(value)),
};

function AppLayout({
  primaryNavigation,
  secondaryNavigation,
  topbar,
  children,
  surface = "separate",
  sidebarWidthStorageKey,
  focusMode = false,
  workspaceControls,
  surfaceProps,
  hideSecondaryNavigation = false,
  primaryNavigationProps,
  secondaryNavigationProps,
  topbarProps,
  contentProps,
  className,
  style,
  ...props
}: AppLayoutProps) {
  const hasSecondaryNavigation = secondaryNavigation !== undefined && secondaryNavigation !== null && secondaryNavigation !== false;
  const showSecondaryNavigation = hasSecondaryNavigation && !hideSecondaryNavigation;
  const joined = surface === "joined";

  const [sidebarWidth, setSidebarWidth] = useLocalStorageState(sidebarWidthStorageKey, undefined, sidebarWidthCodec);
  const drag = React.useRef<{ pointerId: number; x: number; width: number } | null>(null);
  const width = sidebarWidth ?? defaultSidebarWidth;
  const resize = (value?: number) => setSidebarWidth(value === undefined ? undefined : clampSidebarWidth(value));

  return (
    <AppShell
      data-slot="app-layout"
      className={cn(
        "relative [--app-layout-height:100svh] grid-cols-[3rem_minmax(0,1fr)]",
        joined
          ? cn(
            "h-[var(--app-layout-height)] min-h-0 grid-rows-[auto_minmax(0,1fr)]",
            showSecondaryNavigation && "md:grid-cols-[3rem_var(--app-sidebar-width)_minmax(0,1fr)]",
          )
          : cn(
            "min-h-[var(--app-layout-height)]",
            showSecondaryNavigation
              ? "grid-rows-[auto_auto_1fr] md:grid-cols-[3rem_var(--app-sidebar-width)_minmax(0,1fr)] md:grid-rows-[auto_1fr]"
              : "grid-rows-[auto_1fr]",
          ),
        focusMode && "h-[var(--app-layout-height)] min-h-0 grid-cols-[minmax(0,1fr)] grid-rows-[minmax(0,1fr)] md:grid-cols-[minmax(0,1fr)] md:grid-rows-[minmax(0,1fr)]",
        focusMode && !joined && showSecondaryNavigation && "grid-rows-[auto_minmax(0,1fr)] md:grid-cols-[var(--app-sidebar-width)_minmax(0,1fr)] md:grid-rows-[minmax(0,1fr)]",
        className,
      )}
      {...props}
      style={{ ...style, "--app-sidebar-width": `${width}px` } as React.CSSProperties}
    >
      {workspaceControls && (
        <div data-slot="app-workspace-controls" className={cn("pointer-events-none absolute top-11 left-12 z-50", focusMode && "top-0 left-0")}>
          {workspaceControls}
        </div>
      )}
      <AppSidebar
        {...primaryNavigationProps}
        className={cn(
          "col-start-1 row-start-1 h-[var(--app-layout-height)]",
          joined ? "row-span-2" : showSecondaryNavigation && "row-span-3 md:row-span-2",
          primaryNavigationProps?.className,
          focusMode && "hidden",
        )}
      >
        {primaryNavigation}
      </AppSidebar>
      <AppTopbar
        {...topbarProps}
        className={cn(
          "col-start-2 row-start-1",
          showSecondaryNavigation && "md:col-start-3",
          topbarProps?.className,
          focusMode && "hidden",
        )}
      >
        {topbar}
      </AppTopbar>
      <div
        data-slot="app-layout-surface"
        {...surfaceProps}
        className={cn(
          joined
            ? cn(
              "col-start-2 row-start-2 mr-3 mb-3 grid min-h-0 min-w-0 self-stretch overflow-hidden rounded-xl bg-background text-foreground md:mr-4 md:mb-4",
              showSecondaryNavigation
                ? "grid-cols-[minmax(0,1fr)] grid-rows-[auto_minmax(0,1fr)] md:col-span-2 md:grid-cols-[var(--app-sidebar-width)_minmax(0,1fr)] md:grid-rows-[minmax(0,1fr)]"
                : "grid-cols-[minmax(0,1fr)] grid-rows-[minmax(0,1fr)]",
            )
            : "contents",
          surfaceProps?.className,
          focusMode && joined && "col-span-1 col-start-1 row-start-1 m-0 rounded-none md:col-span-1 md:m-0",
        )}
      >
        {hasSecondaryNavigation && (
          <div
            data-slot="app-secondary-navigation"
            {...secondaryNavigationProps}
            hidden={!showSecondaryNavigation}
            className={cn(
              joined
                ? "col-start-1 row-start-1 z-40 min-h-0 min-w-0 self-stretch bg-background px-3 pb-3 md:z-10 md:overflow-hidden md:pt-3"
                : "static col-start-2 row-start-2 z-40 min-w-0 self-start bg-transparent px-3 pb-3 md:sticky md:top-8 md:col-start-2 md:row-start-1 md:row-span-2 md:z-10 md:pt-3",
              joined && "relative",
              secondaryNavigationProps?.className,
              !showSecondaryNavigation && "hidden",
              focusMode && !joined && "col-start-1 row-start-1 md:top-0 md:col-start-1 md:row-start-1 md:row-span-1",
            )}
          >
            {secondaryNavigation}
            <div
              data-slot="app-sidebar-resize-handle"
              role="separator"
              aria-label="Resize sidebar"
              aria-orientation="vertical"
              aria-valuemin={160}
              aria-valuemax={480}
              aria-valuenow={width}
              tabIndex={showSecondaryNavigation ? 0 : -1}
              className="absolute inset-y-0 right-0 z-50 hidden w-2 touch-none cursor-col-resize select-none outline-none after:absolute after:inset-y-0 after:right-0 after:w-0.5 after:bg-ring after:opacity-0 hover:after:opacity-60 focus-visible:after:opacity-100 active:after:opacity-100 md:block"
              onPointerDown={(event) => {
                if (event.button !== 0 || !event.isPrimary) return;
                event.preventDefault();
                event.currentTarget.focus({ preventScroll: true });
                event.currentTarget.setPointerCapture(event.pointerId);
                drag.current = { pointerId: event.pointerId, x: event.clientX, width };
              }}
              onPointerMove={(event) => {
                const start = drag.current;
                if (start?.pointerId === event.pointerId) resize(start.width + event.clientX - start.x);
              }}
              onPointerUp={(event) => {
                if (event.currentTarget.hasPointerCapture(event.pointerId)) {
                  event.currentTarget.releasePointerCapture(event.pointerId);
                }
                drag.current = null;
              }}
              onPointerCancel={() => { drag.current = null; }}
              onLostPointerCapture={() => { drag.current = null; }}
              onDoubleClick={() => resize()}
              onKeyDown={(event) => {
                const next = event.key === "ArrowLeft" ? width - 16
                  : event.key === "ArrowRight" ? width + 16
                  : event.key === "Home" ? 160
                  : event.key === "End" ? 480 : null;
                if (next === null) return;
                event.preventDefault();
                resize(next);
              }}
            />
          </div>
        )}
        <AppContent
          {...contentProps}
          className={cn(
            joined
              ? cn(
                "col-start-1 min-h-0 self-stretch overflow-auto rounded-none border-0 bg-transparent outline-none",
                showSecondaryNavigation ? "row-start-2 md:col-start-2 md:row-start-1" : "row-start-1",
              )
              : cn(
                "col-start-2 mr-3 mb-3 self-stretch scroll-mt-36 outline-none md:mr-4 md:mb-4 md:scroll-mt-8",
                showSecondaryNavigation ? "row-start-3 md:col-start-3 md:row-start-2" : "row-start-2",
              ),
            contentProps?.className,
            focusMode && !joined && "col-start-1 row-start-1 m-0 min-h-0 overflow-auto rounded-none md:col-start-1 md:row-start-1 md:m-0",
            focusMode && !joined && showSecondaryNavigation && "row-start-2 md:col-start-2 md:row-start-1",
          )}
        >
          {children}
        </AppContent>
      </div>
    </AppShell>
  );
}

export { AppLayout, type AppLayoutProps };
