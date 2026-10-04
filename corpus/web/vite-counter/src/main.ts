// The page's entry (brief 0038): wires the counter button. Vite serves this TypeScript transformed, with an inline
// source map, so a breakpoint in src/counter.ts binds through the dev server.
import { setupCounter } from "./counter";

setupCounter(document.querySelector<HTMLButtonElement>("#counter")!);
