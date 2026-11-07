import { MessageSquare, Settings, Store, type LucideIcon } from "lucide-react";
import { NavLink } from "react-router-dom";

import { cn } from "@design-system/ui";

import { AdiLogo } from "./AdiLogo";
import { appHref } from "../lib/app-api";

const pages = [
  { to: "/interface", label: "Chats", icon: MessageSquare },
  { to: "/marketplace", label: "Marketplace", icon: Store },
];

// Phone widths show a bottom tab: icon over label, with colour alone marking the active tab.
// Desktop keeps the compact rail button with its tinted active square.
function tabClass(isActive: boolean) {
  return cn(
    "focus-visible:ring-ring flex min-w-0 flex-1 flex-col items-center justify-center gap-1 rounded-md pt-1.5 pb-1 text-[11px] leading-none font-medium outline-none transition-colors focus-visible:ring-2 max-md:focus-visible:ring-inset md:size-9 md:flex-none md:gap-0 md:rounded-lg md:p-0",
    isActive ? "text-foreground md:bg-foreground/10" : "text-muted-foreground hover:text-foreground md:hover:bg-foreground/5",
  );
}

function TabIcon({ icon: Icon, active, className }: { icon: LucideIcon; active: boolean; className?: string }) {
  return <Icon aria-hidden="true" className={cn("size-6 md:size-4", active && "fill-current/15 stroke-[2.25] md:fill-none md:stroke-2", className)} />;
}

export function ShowcaseNavigation() {
  return (
    <nav data-slot="global-navigation" aria-label="Main navigation" className="flex h-full w-full items-stretch justify-around px-2 pb-[env(safe-area-inset-bottom)] md:w-12 md:flex-col md:items-center md:justify-start md:px-0 md:pb-4">
      <div className="flex flex-1 items-stretch justify-center md:h-11 md:w-full md:flex-none md:items-center">
        <NavLink to="/" end aria-label="ADI home" title="ADI home" className={({ isActive }) => tabClass(isActive)}>
          {/* The ADI mark is the Home icon on every width; it follows the tab colour. */}
          <AdiLogo className="size-6 md:size-5" />
          <span className="md:sr-only">Home</span>
        </NavLink>
      </div>
      <div className="contents md:flex md:flex-col md:items-center md:gap-1">
        {pages.map(({ to, label, icon }) => (
          <NavLink key={to} to={to} end={to !== "/marketplace"} aria-label={label} title={label} className={({ isActive }) => tabClass(isActive)}>
            {({ isActive }) => (
              <>
                <TabIcon icon={icon} active={isActive} />
                <span className="md:sr-only">{label}</span>
              </>
            )}
          </NavLink>
        ))}
      </div>
      <a href={appHref("/extended/settings")} aria-label="Settings" title="Settings" className={cn(tabClass(false), "md:mt-auto md:shrink-0")}>
        <TabIcon icon={Settings} active={false} />
        <span className="md:sr-only">Settings</span>
      </a>
    </nav>
  );
}
