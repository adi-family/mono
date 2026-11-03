import { type ClassValue, clsx } from "clsx";
import { twMerge } from "tailwind-merge";

// shadcn/ui's own helper: merges conditional class names (`clsx`) and then resolves conflicting
// Tailwind utilities in favour of the last one (`tailwind-merge`) — every component below uses
// this instead of string-concatenating `className`.
export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}
