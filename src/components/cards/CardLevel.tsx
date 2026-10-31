"use client";

// ---------------------------------------------------------------------------
// A card as a place in the Cards page, in the review page's livery.
//
// Clicking a row used to raise the editor overlay: a card had no address in the
// page, so the trail could only say `Cards › der Lernende` and the card itself
// was a modal you had to dismiss to get back to the list you came from. This
// level puts the card in the content area behind a third crumb, so a card is a
// step in the trail like a dataset is.
//
// The display is the review page's, not the editor's. That page already settled
// what a card looks like when nobody is editing it: a raised surface, the
// question as a centred display heading, the answer as prose at reading size,
// notes small underneath, and the grade scale held in its own row above so a
// long answer scrolls under it. The overlay's `CardFields` is the other half of
// that pair —
// labels, boxes, a pencil per field — and it is right for a form and wrong for a
// page. Copying the review livery here also means a card looks the same whether
// you are being tested on it or reading it, which is the same card twice.
//
// The difference is that this page does not flip, so the faces stand side by
// side down the page instead of inside one box: the prompt gets the review
// page's front surface, the answer gets its back surface (without the prompt
// repeated — here it is still on screen, one scroll up), and the two things the
// back side sets small under an answer — what the card is filed under, and what
// the reader wrote about it — get surfaces of their own rather than borrowing
// the answer's, because neither is part of the answer.
//
// What the level adds that the review page cannot: a bookmark's metadata (what
// it is filed under, how practised, when changed), voice (the card read aloud
// in the deck's language), size (an image opens at screen size), and `Edit`,
// which hands the same card back to the overlay so draft, save, the exit guard,
// attachments and delete stay in the one place that owns them.
// ---------------------------------------------------------------------------

import { useEffect, useState } from "react";
import { Check } from "lucide-react";
import type { Card, CardMedia, CardReview } from "@/lib/types";
import { stripAnchors } from "@/lib/media/anchors";
import MediaField from "./MediaField";
import CardGradeScale, { CARD_GRADES } from "./CardGradeScale";
import SpeakButton from "./SpeakButton";

type Props = {
  card: Card;
  media: CardMedia[];
  /** Absolute dataset directory, for resolving attachment thumbnails. */
  datasetPath: string;
  /** The overlay, on this card. The toolbar owns the button; the level is what
   *  the reader is looking at while they press it. */
  onEdit: () => void;
  /** The deck's language, for reading the card aloud; "" = app preferred voice. */
  language: string;
  /** Grade the card; resolves to what the scheduler stored, or null if the
   *  rating did not happen (already in flight, no deck, write failed). */
  onRate: (quality: number) => Promise<CardReview | null>;
};

export default function CardLevel({
  card,
  media,
  datasetPath,
  onEdit,
  language,
  onRate,
}: Props) {
  const [busy, setBusy] = useState(false);
  // The last grade given to *this* card, so the reader is told what the click
  // did — a bare familiarity number is not an answer. Cleared when another card
  // arrives, or the feedback would follow you through the deck.
  const [rated, setRated] = useState<{ quality: number; review: CardReview } | null>(null);
  useEffect(() => setRated(null), [card.uuid]);

  const grade = async (quality: number) => {
    if (busy) return;
    setBusy(true);
    const review = await onRate(quality);
    setBusy(false);
    if (review) setRated({ quality, review });
  };

  const question = stripAnchors(card.question).trim();
  const suggestion = stripAnchors(card.suggestion).trim();
  // One utterance for the card: the prompt, then what it is asking for. Two
  // speakers on one page would be two buttons for one act — listening to it.
  const spoken = [card.question, card.answer].filter((s) => s.trim()).join("\n\n");

  return (
    <div className="flex flex-1 min-h-0 flex-col">
      {/* The grade row is a row above the scroller, not a sticky bar inside it.
          Sticky leaves the scroll container's own padding over the bar
          uncovered, so a long answer's text ran through that band and read as a
          gap between this row and the toolbar. Outside the scroller there is
          nothing above the row to show through, at any scroll position. */}
      <div className="shrink-0 bg-bg-body px-3 pt-3 pb-4 sm:px-6 sm:pt-6">
        <div className="flex flex-wrap justify-center gap-2">
          <CardGradeScale onRate={grade} disabled={busy} />
        </div>
      </div>

      <div className="flex-1 min-h-0 overflow-y-auto px-3 pb-3 sm:px-6 sm:pb-6">
        {/* The line the review page spends naming the source deck, spent here on
            what a reader needs to place the card: how practised it is, when it
            last changed — and the one act that is not reading it silently. What
            the card is filed under sits with the answer, because that is where
            the card's own back side would put it. */}
        <div className="mb-2 flex flex-wrap items-center gap-x-3 gap-y-1.5 text-xs text-text-tertiary">
          <span title="Familiarity, 0–6">f={card.familiarity}</span>
          <span>updated {card.updated_at.slice(0, 16).replace("T", " ")}</span>
          <span className="flex-1" />
          {spoken && <SpeakButton text={spoken} language={language} label="Card" />}
        </div>

        {/* The prompt, in the review page's front treatment: its own surface, the
            question as a centred display heading, `my-auto` holding a short card
            in the middle of the box. */}
        <div className="flex min-h-[220px] flex-col rounded-xl border border-border-default bg-bg-card p-4 sm:min-h-[300px] sm:p-8">
          <div className="my-auto w-full break-words text-center text-4xl font-bold sm:text-5xl">
            {question ? (
              <MediaField
                text={card.question}
                media={media}
                datasetPath={datasetPath}
                mode="plain"
                zoomable
              />
            ) : (
              <span className="text-text-tertiary">(empty question)</span>
            )}
          </div>
        </div>

        {/* The answer, in the review page's back treatment minus the repeated
            prompt: its own surface, prose at reading size, and nothing else — a
            suggestion is not part of an answer, and neither is a note. */}
        <div className="mt-4 flex min-h-[220px] flex-col rounded-xl border border-border-default bg-bg-card p-4 sm:min-h-[300px] sm:p-8">
          <div className="my-auto w-full break-words">
            <div className="text-xl">
              {card.answer.trim() ? (
                <MediaField
                  text={card.answer}
                  media={media}
                  datasetPath={datasetPath}
                  mode="markdown"
                  zoomable
                />
              ) : (
                <span className="italic text-text-tertiary">
                  No answer yet —{" "}
                  <button
                    type="button"
                    onClick={onEdit}
                    className="underline hover:text-text-secondary"
                  >
                    write one
                  </button>
                  .
                </span>
              )}
            </div>
          </div>
        </div>

        {/* What the card is filed under, on its own surface: one word (a
            category, not a sentence) should not be inside the answer's card
            pretending to be part of the answer. */}
        {suggestion && (
          <div className="mt-4 rounded-xl border border-border-default bg-bg-card px-4 py-3 sm:px-8 sm:py-4">
            <span className="rounded bg-mid-gray/20 px-1.5 py-0.5 text-xs text-text-secondary">
              {suggestion}
            </span>
          </div>
        )}

        {/* The reader's own aside, on its own surface too — small, because it is
            about the card rather than the answer. */}
        {card.note.trim() && (
          <div className="mt-4 rounded-xl border border-border-default bg-bg-card px-4 py-3 text-sm sm:px-8 sm:py-4">
            <MediaField
              text={card.note}
              media={media}
              datasetPath={datasetPath}
              mode="markdown"
              zoomable
            />
          </div>
        )}

        {/* The review page's feedback, in its own colours and words: what the
            click did, and when the card comes back. */}
        {rated && (
          <div className="mt-4 rounded-lg bg-green-500/10 p-3 text-center text-sm text-green-600">
            <Check size={16} className="mr-1 inline" />
            Rated {CARD_GRADES.find((g) => g.q === rated.quality)?.label}. Next review
            in {rated.review.interval_days} day(s). Familiarity: {rated.review.familiarity}
          </div>
        )}
      </div>
    </div>
  );
}
