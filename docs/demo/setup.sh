# Builds the demo project in a temporary folder and goes there: examples/shop, with the 9 laws an
# agent drafted through `tinstar mcp` in Zed (DeepSeek model), approved. Sourced by terminal.tape.
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
demo="$(mktemp -d)/shop"
mkdir -p "$demo"
(cd "$repo/examples/shop" && tar --exclude='./.tinstar/laws/*.yaml' -cf - .) | tar -xf - -C "$demo"
cp "$repo"/docs/demo/laws/*.yaml "$demo/.tinstar/laws/"
cd "$demo" || return
export PS1='\[\e[1;35m\]shop\[\e[0m\] $ '
# What Claude Code sends the hook after an edit (trimmed to what tinstar reads).
export EDIT='{"tool_name":"Edit","tool_input":{"file_path":"src/cart.ts"}}'
