import type { Metadata } from "next";
import MarketingPage from "@/components/site/MarketingPage";

export const metadata: Metadata = {
  title: "FmS — learn a language by listening",
  description:
    "FmS turns any audiobook, podcast or course into listening, reading and speaking practice — dictation, books, read-aloud scoring, spaced-repetition cards, wiki notes and an agent-friendly built-in MCP server. Free and open source for Windows and Linux.",
};

export default function SiteHome() {
  return <MarketingPage />;
}
