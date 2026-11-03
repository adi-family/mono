import * as React from "react";
import { Moon, Sun } from "lucide-react";

import { Button, type ButtonDisabledProps } from "./button";

type Theme = "light" | "dark";

type ThemeToggleProps = Omit<
  React.ComponentProps<typeof Button>,
  | "asChild"
  | "disabled"
  | "disabledReason"
  | "children"
  | "type"
  | "role"
  | "aria-checked"
  | "aria-label"
  | "aria-labelledby"
  | "aria-pressed"
> & ButtonDisabledProps & {
  theme: Theme;
  onThemeChange: (theme: Theme) => void;
};

function ThemeToggle({
  theme,
  onThemeChange,
  onClick,
  variant = "outline",
  ...props
}: ThemeToggleProps) {
  const isDark = theme === "dark";
  const Icon = isDark ? Moon : Sun;

  return (
    <Button
      {...props}
      variant={variant}
      type="button"
      data-slot="theme-toggle"
      role="switch"
      aria-checked={isDark}
      aria-label="Dark theme"
      onClick={(event) => {
        onClick?.(event);
        if (!event.defaultPrevented) {
          onThemeChange(isDark ? "light" : "dark");
        }
      }}
    >
      <Icon className="size-4" aria-hidden="true" />
      <span>{isDark ? "Dark" : "Light"}</span>
    </Button>
  );
}

export { ThemeToggle, type Theme, type ThemeToggleProps };
