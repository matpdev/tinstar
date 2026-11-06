import { Money } from "../money";

export async function charge(amount: Money, token: string): Promise<string> {
  const res = await fetch("https://api.payments.example.com/v1/charges", {
    method: "POST",
    body: JSON.stringify({ amount: amount.cents, token }),
  });
  return (await res.json()).id;
}
