// Brief 0050's ESLint fixture: `total` is never reassigned, so prefer-const reports it and fixes it to `const`.
let total = 1;
console.log(total);
