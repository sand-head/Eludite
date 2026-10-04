// The page's script (brief 0038): the "Add" button's handler adds the price typed to the cart and shows the total.
// It has a bug for a breakpoint to find: `total` skips the first item, so the first Add shows 0.
// Compiled once to app.js and app.js.map (corpus/README.md names the command); the tests need no TypeScript.

interface Item {
  name: string;
  price: number;
}

const cart: Item[] = [];

function total(items: Item[]): number {
  let sum = 0;
  for (let i = 1; i < items.length; i++) {
    sum += items[i].price;
  }
  return sum;
}

function onAdd(): void {
  const input = document.getElementById("price") as HTMLInputElement;
  const price = Number(input.value);
  const item: Item = { name: `item ${cart.length + 1}`, price };
  cart.push(item);
  const sum = total(cart);
  document.getElementById("total")!.textContent = String(sum);
}

document.getElementById("add")!.addEventListener("click", onAdd);
