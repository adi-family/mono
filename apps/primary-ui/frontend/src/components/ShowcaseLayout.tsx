import { useLayoutEffect } from "react";
import { ChevronRight } from "lucide-react";
import { Outlet, useLocation } from "react-router-dom";

import { AppLayout, ThemeToggle } from "@design-system/ui";

import { useTheme } from "../lib/theme";
import { ComponentSidebar } from "./ComponentSidebar";
import { ShowcaseNavigation } from "./ShowcaseNavigation";
import { AppChatSidebar } from "./AppChatSidebar";

export function ShowcaseLayout() {
  const { pathname, hash, key } = useLocation();
  const { theme, setTheme } = useTheme();
  const isHome = pathname === "/";
  const isInterface = pathname === "/interface";
  const title = isHome
    ? "Home"
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
      secondaryNavigation={isInterface ? <AppChatSidebar /> : <ComponentSidebar />}
      secondaryNavigationProps={isInterface ? { className: "bg-muted/50 dark:bg-background" } : undefined}
      hideSecondaryNavigation={isHome}
      topbar={(
        <>
          <div className="flex min-w-0 items-center gap-2">
            <span className="text-muted-foreground hidden text-sm sm:inline">{isHome || isInterface ? "ADI" : "Design System"}</span>
            <ChevronRight aria-hidden="true" className="text-muted-foreground hidden size-3.5 sm:block" />
            <h1 className="truncate text-sm font-medium">{title}</h1>
          </div>
          <ThemeToggle theme={theme} onThemeChange={setTheme} />
        </>
      )}
      contentProps={{ id: "page-content", tabIndex: -1, className: isInterface ? "flex min-h-0 flex-col overflow-hidden p-0 sm:p-0" : isHome ? "flex" : undefined }}
    >
      <Outlet context={{ theme, setTheme }} />
    </AppLayout>
  );
}
