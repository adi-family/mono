import { type ClassValue, clsx } from "clsx";
import { twMerge } from "tailwind-merge";

// Merge conditional class names (clsx) then resolve conflicting Tailwind utilities in favour of
// the last one (tailwind-merge). Every component uses this instead of concatenating className.
export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}
