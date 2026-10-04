// The counter (brief 0038): each click adds one. A breakpoint on the `counter = count;` line stops on a click.
export function setupCounter(element: HTMLButtonElement): void {
  let counter = 0;
  const setCounter = (count: number) => {
    counter = count;
    element.innerHTML = `count is ${counter}`;
  };
  element.addEventListener("click", () => setCounter(counter + 1));
  setCounter(0);
}
