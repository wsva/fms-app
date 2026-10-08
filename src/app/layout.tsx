import type { Metadata, Viewport } from "next";
import "./globals.css";
import Providers from "./providers";

export const metadata: Metadata = {
  title: "fms-app",
  description: "A Tauri App",
};

// viewport-fit=cover lets the Android WebView report safe-area insets so the
// bottom navigation can pad itself clear of the system gesture bar.
export const viewport: Viewport = {
  width: "device-width",
  initialScale: 1,
  viewportFit: "cover",
};

// The `data-mobile` tag in the head script below must keep the same test as
// isMobileApp() in src/lib/platform.ts. It exists because the static export is
// prerendered once (as the desktop shell), so only a script that runs before
// first paint can tell the Android boot splash apart from a desktop session —
// React's own deferred platform flag arrives too late. See .app-boot-splash in
// globals.css and the `booted` state in page.tsx.

// pageZoom is read here as well as in usePageZoom() so a restored zoom level
// lands before first paint instead of jumping — same anti-flash reason as the
// theme attribute, and the key plus the 0.5–2 clamp must stay in sync with
// src/hooks/usePageZoom.ts.

export default function RootLayout({
  children,
}: {
  children: React.ReactNode;
}) {
  return (
    <html lang="en" suppressHydrationWarning>
      <head>
        <script dangerouslySetInnerHTML={{ __html: `
          (function(){var t=localStorage.getItem('theme');if(t&&t!=='light')document.documentElement.setAttribute('data-theme',t);if(/android/i.test(navigator.userAgent))document.documentElement.setAttribute('data-mobile','1');var z=parseFloat(localStorage.getItem('pageZoom'));if(z>=0.5&&z<=2&&z!==1)document.documentElement.style.zoom=String(z);})()
        `}} />
      </head>
      <body>
        <Providers>{children}</Providers>
      </body>
    </html>
  );
}
