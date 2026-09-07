import type { Metadata } from "next";
import "./globals.css";

export const metadata: Metadata = {
  title: "fms-app",
  description: "A Tauri App",
};

export default function RootLayout({
  children,
}: {
  children: React.ReactNode;
}) {
  return (
    <html lang="en" suppressHydrationWarning>
      <head>
        <script dangerouslySetInnerHTML={{ __html: `
          (function(){var t=localStorage.getItem('theme');if(t&&t!=='light')document.documentElement.setAttribute('data-theme',t);})()
        `}} />
      </head>
      <body>{children}</body>
    </html>
  );
}
