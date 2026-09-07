"use client";

export default function Error({
  error,
  reset,
}: {
  error: Error & { digest?: string };
  reset: () => void;
}) {
  return (
    <div className="app-main">
      <h2>Something went wrong</h2>
      <p>{error.message}</p>
      <button onClick={reset} className="app-btn primary" style={{ marginTop: "1em" }}>
        Try again
      </button>
    </div>
  );
}
