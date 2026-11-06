# Shop: rules for anyone changing this code (people and AI agents)

## Money
Money is always an integer number of cents (the `Money` type), never a floating point `number`.
Prices, totals, amounts, costs and discounts are `Money`.

## Logging
Code under `src/` logs through `logger`, never `console.log`.

## Payments
Only `src/payments/` talks to the payment provider's API.

## Features
Every feature folder in `src/features/` has an `index.ts` and at least one test.

## Containers
Every container in a Kubernetes Deployment declares resource limits.
Images are pinned to a version: never `latest`, never without a tag.

## CI
GitHub Actions are pinned to a commit SHA, not to a tag.

## Secrets
Secrets never go into the repository, so no `.env` file is committed.

## Database
Migrations are named `YYYYMMDD_description.sql`, in lowercase.

## Things a checker can't see
Discounts never exceed 30% of the order total. (A runtime rule: it stays here, for people and agents.)
