import { Money } from "./money";
import { logger } from "./logger";

export type Item = { sku: string; price: Money; quantity: number };

export function subtotal(items: Item[]): Money {
  return items.reduce((sum, i) => Money.add(sum, Money.cents(i.price.cents * i.quantity)), Money.cents(0));
}

export function applyCoupon(items: Item[], code: string): Money {
  let discount: number = code === "WELCOME" ? 0.1 : 0;
  console.log("coupon", code, discount);
  const total = subtotal(items);
  logger.info("total", total.cents);
  return Money.cents(total.cents * (1 - discount));
}
