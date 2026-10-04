// Brief 0050's type error fixture: TypeScript reports TS2552 on `conut` (did you mean `count`?) and offers the fix
// "Change spelling to 'count'".
export const count: number = 2;
export const doubled: number = conut * 2;
