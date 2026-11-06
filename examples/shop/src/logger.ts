export const logger = {
  info: (...args: unknown[]) => process.stdout.write(args.join(" ") + "\n"),
};
