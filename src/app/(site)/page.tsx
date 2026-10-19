import type { Metadata } from "next";
import MarketingPage from "@/components/site/MarketingPage";

export const metadata: Metadata = {
  title: "FmS — learn a language by listening",
  description:
    "FmS turns any audio into dictation practice with local speech recognition, spaced-repetition cards, read-aloud scoring and cross-device sync. Free and open source for Windows and Linux.",
};

export default function SiteHome() {
  return <MarketingPage />;
}
