import { MessageSquare, Settings, Store } from "lucide-react";
import { NavLink } from "react-router-dom";

import { cn } from "@design-system/ui";

import { AdiLogo } from "./AdiLogo";
import { appHref } from "../lib/app-api";

const pages = [
  { to: "/interface", label: "Chats", icon: MessageSquare },
  { to: "/marketplace", label: "Marketplace", icon: Store },
];

export function ShowcaseNavigation() {
  return (
    <nav data-slot="global-navigation" aria-label="Main navigation" className="flex h-full w-12 flex-col items-center pb-3 md:pb-4">
      <div className="flex h-11 w-full items-center justify-center">
        <NavLink
          to="/"
          end
          aria-label="ADI home"
          title="ADI home"
          className={({ isActive }) => cn(
            "focus-visible:ring-ring flex size-9 items-center justify-center rounded-lg outline-none transition-colors focus-visible:ring-2",
            isActive ? "bg-foreground/10 text-foreground" : "text-muted-foreground hover:bg-foreground/5 hover:text-foreground",
          )}
        >
          <AdiLogo className="size-5" />
          <span className="sr-only">ADI home</span>
        </NavLink>
      </div>
      <div className="flex flex-col items-center gap-1">
        {pages.map(({ to, label, icon: Icon }) => (
          <NavLink
            key={to}
            to={to}
            end
            aria-label={label}
            title={label}
            className={({ isActive }) => cn(
              "focus-visible:ring-ring flex size-9 items-center justify-center rounded-lg outline-none transition-colors focus-visible:ring-2",
              isActive ? "bg-foreground/10 text-foreground" : "text-muted-foreground hover:bg-foreground/5 hover:text-foreground",
            )}
          >
            <Icon className="size-4" aria-hidden="true" />
            <span className="sr-only">{label}</span>
          </NavLink>
        ))}
      </div>
      <a
        href={appHref("/extended/settings")}
        aria-label="Settings"
        title="Settings"
        className="text-muted-foreground hover:bg-foreground/5 hover:text-foreground focus-visible:ring-ring mt-auto flex size-9 shrink-0 items-center justify-center rounded-lg outline-none transition-colors focus-visible:ring-2"
      >
        <Settings className="size-4" aria-hidden="true" />
        <span className="sr-only">Settings</span>
      </a>
    </nav>
  );
}
