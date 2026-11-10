import { useEffect, useState } from "react";

// Matches index.css's mobile breakpoint, below which the rail becomes a bottom tab bar and the
// chat page swaps the topbar for each screen's own header.
export const mobileQuery = "(max-width: 767px)";

export function useIsMobile() {
  const [mobile, setMobile] = useState(() => window.matchMedia(mobileQuery).matches);
  useEffect(() => {
    const media = window.matchMedia(mobileQuery);
    const update = () => setMobile(media.matches);
    media.addEventListener("change", update);
    return () => media.removeEventListener("change", update);
  }, []);
  return mobile;
}
