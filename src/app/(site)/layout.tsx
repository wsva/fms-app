import { LocaleProvider } from "@/lib/i18n";

// Layout for the public marketing site (`(site)` route group). It only adds the
// client-side locale provider; the app chrome lives in the `(app)` group's
// shell, so the landing page gets a full-viewport bare layout.
export default function SiteLayout({
  children,
}: {
  children: React.ReactNode;
}) {
  return <LocaleProvider>{children}</LocaleProvider>;
}
