import { Money } from "../../money";

test("cents are integers", () => {
  expect(Money.cents(10.4).cents).toBe(10);
});
