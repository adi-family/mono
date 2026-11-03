import { useId, type ComponentProps } from "react";

import { cn } from "../lib/utils";

// Exact monochrome Cut mark from adi-family/crates/adi-ui/src/mark.rs.
export function AdiLogo({ className, ...props }: ComponentProps<"svg">) {
  const id = useId();
  const backMask = `${id}-back`;
  const middleMask = `${id}-middle`;

  return (
    <svg
      {...props}
      className={cn("size-6 shrink-0", className)}
      viewBox="0 0 200 200"
      fill="none"
      aria-hidden="true"
      focusable="false"
    >
      <defs>
        <mask id={backMask} maskUnits="userSpaceOnUse" x="0" y="0" width="200" height="200">
          <rect width="200" height="200" fill="#fff" />
          <path fill="#000" d="M127.71 65.00 L178.81 94.50 L178.81 153.50 L127.71 183.00 L76.62 153.50 L76.62 94.50 Z" />
          <path fill="#000" d="M100.00 17.00 L151.10 46.50 L151.10 105.50 L100.00 135.00 L48.90 105.50 L48.90 46.50 Z" />
        </mask>
        <mask id={middleMask} maskUnits="userSpaceOnUse" x="0" y="0" width="200" height="200">
          <rect width="200" height="200" fill="#fff" />
          <path fill="#000" d="M100.00 17.00 L151.10 46.50 L151.10 105.50 L100.00 135.00 L48.90 105.50 L48.90 46.50 Z" />
        </mask>
      </defs>
      <path fill="currentColor" fillOpacity="0.52" mask={`url(#${backMask})`} d="M72.29 68.00 L120.78 96.00 L120.78 152.00 L72.29 180.00 L23.79 152.00 L23.79 96.00 Z" />
      <path fill="currentColor" fillOpacity="0.74" mask={`url(#${middleMask})`} d="M127.71 68.00 L176.21 96.00 L176.21 152.00 L127.71 180.00 L79.22 152.00 L79.22 96.00 Z" />
      <path fill="currentColor" d="M100.00 20.00 L148.50 48.00 L148.50 104.00 L100.00 132.00 L51.50 104.00 L51.50 48.00 Z" />
    </svg>
  );
}
