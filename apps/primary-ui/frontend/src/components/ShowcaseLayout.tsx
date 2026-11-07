import { useLayoutEffect, useState } from "react";
import { ChevronRight } from "lucide-react";
import { Outlet, useLocation } from "react-router-dom";

import { AppLayout, Button, ThemeToggle } from "@design-system/ui";

import { useTheme } from "../lib/theme";
import { ComponentSidebar } from "./ComponentSidebar";
import { ShowcaseNavigation } from "./ShowcaseNavigation";
import { AppChatSidebar } from "./AppChatSidebar";

export function ShowcaseLayout() {
  const [focusMode, setFocusMode] = useState(false);
  const { pathname, hash, key } = useLocation();
  const { theme, setTheme } = useTheme();
  const isHome = pathname === "/";
  const isMarketplace = (pathname === "/marketplace" || pathname.startsWith("/marketplace/"));
  const isInterface = pathname === "/interface";
  const title = isHome
    ? "Home"
    : isMarketplace
      ? "Marketplace"
    : pathname === "/library/agents/layout-reviewer"
      ? "Layout reviewer"
      : pathname === "/library"
        ? "Components"
        : "Chats";

  useLayoutEffect(() => {
    if (!hash) window.scrollTo(0, 0);
  }, [pathname, hash, key]);

  return (
    <AppLayout
      focusMode={focusMode}
      sidebarWidthStorageKey="adi.sidebar-width.v1"
      lang="en"
      className={isInterface ? "overflow-hidden" : undefined}
      surface={isInterface ? "joined" : "separate"}
      surfaceProps={isInterface ? { className: "dark:bg-black" } : undefined}
      primaryNavigation={(
        <>
          <a href="#page-content" className="bg-background focus:not-sr-only focus:fixed focus:top-4 focus:left-4 focus:z-50 focus:rounded-md focus:border focus:p-3 sr-only">Skip to content</a>
          <ShowcaseNavigation />
        </>
      )}
      workspaceControls={(
          <Button
            data-slot="workspace-expand"
            variant="ghost" size="icon"
            className="group text-muted-foreground/60 hover:text-foreground focus-visible:text-foreground pointer-events-auto absolute top-0 left-0 size-5 items-start justify-start rounded-none border-0 bg-transparent p-0 shadow-none hover:bg-transparent"
            aria-label={focusMode ? "Restore navigation" : "Expand workspace"}
            title={focusMode ? "Restore navigation" : "Expand workspace"}
            aria-pressed={focusMode}
            onClick={() => setFocusMode((current) => !current)}
          >
            <svg aria-hidden="true" viewBox="0 0 20 20" className="size-4 origin-top-left fill-current motion-safe:transition-transform motion-safe:duration-150 motion-safe:ease-out motion-safe:group-hover:scale-125 motion-safe:group-focus-visible:scale-125">
              <path d={focusMode ? "M18 2v16H2Z" : "M2 18V2h16Z"} />
            </svg>
          </Button>
      )}
      secondaryNavigation={isInterface ? <AppChatSidebar /> : <ComponentSidebar />}
      secondaryNavigationProps={isInterface ? { className: "bg-muted/50 dark:bg-background" } : undefined}
      hideSecondaryNavigation={isHome || isMarketplace}
      topbar={(
        <>
          <div className="flex min-w-0 items-center gap-2">
            <span className="text-muted-foreground hidden text-sm sm:inline">{isHome || isInterface || isMarketplace ? "ADI" : "Design System"}</span>
            <ChevronRight aria-hidden="true" className="text-muted-foreground hidden size-3.5 sm:block" />
            <h1 className="truncate text-sm font-medium">{title}</h1>
          </div>
          <ThemeToggle size="sm" className="h-7 gap-1.5 px-2 text-xs" theme={theme} onThemeChange={setTheme} />
        </>
      )}
      contentProps={{ id: "page-content", tabIndex: -1, className: isInterface ? "flex min-h-0 flex-col overflow-hidden p-0 sm:p-0" : isHome ? "flex" : undefined }}
    >
      <Outlet context={{ theme, setTheme }} />
    </AppLayout>
  );
}
