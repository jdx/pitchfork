# Directory-change hooks for Bash

This helper implements a `chpwd_functions` array in Bash, following
[Zsh's hook-function convention](https://zsh.sourceforge.io/Doc/Release/Functions.html#Hook-Functions).
Pitchfork loads it as part of Bash activation.

## Load the helper

From the `bash_zsh_support` directory:

```bash
source chpwd/functions.sh
source chpwd/load.sh
```

## Register a hook

Define your function, then add its name to the array once:

```bash
my_directory_hook() {
  printf 'Directory changed to %s\n' "$PWD"
}

declare -a chpwd_functions
if [[ " ${chpwd_functions[*]} " != *" my_directory_hook "* ]]; then
  chpwd_functions+=(my_directory_hook)
fi
```
