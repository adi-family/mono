import * as React from "react";

import { cn } from "../lib/cn";

function AppShell({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="app-shell"
      className={cn(
        "grid min-h-svh min-w-0 grid-cols-[auto_minmax(0,1fr)] grid-rows-[auto_1fr] items-start bg-transparent",
        className,
      )}
      {...props}
    />
  );
}

function AppSidebar({ className, ...props }: React.ComponentProps<"aside">) {
  return (
    <aside
      data-slot="app-sidebar"
      className={cn("sticky top-0 row-span-2 self-start bg-transparent", className)}
      {...props}
    />
  );
}

function AppTopbar({ className, ...props }: React.ComponentProps<"header">) {
  return (
    <header
      data-slot="app-topbar"
      className={cn(
        "flex min-h-11 min-w-0 items-center justify-between gap-3 bg-transparent px-3",
        className,
      )}
      {...props}
    />
  );
}

function AppContent({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="app-content"
      className={cn("min-w-0 rounded-xl border bg-background p-5 sm:p-6", className)}
      {...props}
    />
  );
}

export { AppShell, AppSidebar, AppTopbar, AppContent };
