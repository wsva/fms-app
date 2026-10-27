"use client";

import { useEffect, useState } from "react";
import { isMobileApp } from "@/lib/platform";

/**
 * SSR-safe mobile-platform flag for the toolbar kit.
 *
 * `isMobileApp()` is false during prerender (no `navigator`) and only turns
 * true inside the Android thin-client webview, so resolve it in an effect to
 * avoid a hydration mismatch — the first client paint matches the server HTML.
 *
 * This centralizes the `const [mobile,setMobile]=useState(false); useEffect(...)`
 * boilerplate that each page previously repeated. `BooksPage` keeps sourcing
 * `mobile` from `useBookData` and passes it into `<Toolbar mobile={...}>`.
 */
export function useIsMobile(): boolean {
  const [mobile, setMobile] = useState(false);
  useEffect(() => {
    setMobile(isMobileApp());
  }, []);
  return mobile;
}

export default useIsMobile;
