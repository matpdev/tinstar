export type Money = { readonly cents: number };

export const Money = {
  cents: (cents: number): Money => ({ cents: Math.round(cents) }),
  add: (a: Money, b: Money): Money => ({ cents: a.cents + b.cents }),
};
