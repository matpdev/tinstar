# The terminal side of the demo. From the repository root, with tinstar installed:
#   vhs docs/demo/terminal.tape
Output docs/demo/tinstar-terminal.gif
Output docs/demo/tinstar-terminal.mp4

Set Shell "bash"
Set FontSize 18
Set Width 1440
Set Height 900
Set Padding 28
Set Theme "Catppuccin Mocha"
Set TypingSpeed 40ms
Set WindowBar Colorful
Set CursorBlink false

Hide
Type "source docs/demo/setup.sh && clear"
Enter
Sleep 500ms
Show

# 1. The rules, as people and agents read them
Type "# The project's rules, in plain language (AGENTS.md):"
Enter
Type "sed -n '3,21p' AGENTS.md"
Sleep 300ms
Enter
Sleep 5s
Ctrl+L
Sleep 300ms

# 2. Compiled once into laws
Type "# Compiled into laws once (here by an agent in Zed). No LLM from here on:"
Enter
Type "tinstar laws | head -10"
Sleep 300ms
Enter
Sleep 4500ms
Ctrl+L
Sleep 300ms

# 3. Code
Type "# Checking the code:"
Enter
Type "time tinstar check src/"
Sleep 300ms
Enter
Sleep 7s
Ctrl+L
Sleep 300ms

# 3b. Infra and CI, same engine
Type "# Infra and CI, same way:"
Enter
Type "tinstar check deploy/ .github/"
Sleep 300ms
Enter
Sleep 6s
Ctrl+L
Sleep 300ms

# 4. What an AI agent hears after an edit (Claude Code hook)
Type "# What an AI agent is told right after it edits src/cart.ts (one line per problem):"
Enter
Type 'echo "$EDIT" | tinstar hook claude-code'
Sleep 300ms
Enter
Sleep 8s
