// The one class-name joiner. `twMerge` resolves conflicting Tailwind utilities
// last-wins, so a caller's `className` can override a variant without the result
// depending on stylesheet order — which is what a hand-written template string
// (the convention everywhere else in this app) cannot do.
//
// Use it for component *variants* only: page code keeps template strings, since
// there is nothing to merge there.
import { clsx, type ClassValue } from "clsx";
import { twMerge } from "tailwind-merge";

export function cn(...inputs: ClassValue[]): string {
  return twMerge(clsx(inputs));
}
