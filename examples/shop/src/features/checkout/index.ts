import { Money } from "../../money";

export async function refund(chargeId: string, amount: Money) {
  return fetch(`https://api.payments.example.com/v1/refunds/${chargeId}`, {
    method: "POST",
    body: JSON.stringify({ amount: amount.cents }),
  });
}
