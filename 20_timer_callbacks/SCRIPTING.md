# QEMU shell scripting

The QEMU x86 demo shell has a small C-style interpreter. It runs in the shell's
existing ring-3 address space on a user-eligible CPU, not on reserved CPU 0.
It is not a C compiler, a separate process, or an async executor.

## Interactive scripts

Type `script`, enter one line at each `script>` prompt, then enter `.end` on a
line by itself to execute. `.cancel` discards the input without executing it.

```text
User A [ring 3]$ script
script> let count = 0;
script> while (count < 3) {
script>   print(count);
script>   count = count + 1;
script> }
script> .end
0
1
2
User A [ring 3]$
```

This works without any disk changes. Each invocation starts with fresh variables.
An error stops the script and reports its line and category; previous output is
not rolled back. Script input itself must be at most 4096 bytes, with at most
126 characters per interactive line. Input that exceeds a limit is discarded.

## Language

- Variables: `let name = expression;` and `name = expression;`.
- Signed 64-bit decimal integers, parentheses, unary `-`, and `+`, `-`, `*`, `/`.
  Multiplication/division bind before addition/subtraction. Division truncates
  toward zero; overflow and division by zero are errors.
- One comparison per expression: `==`, `!=`, `<`, `<=`, `>`, `>=`; comparisons
  produce `0` or `1`. Zero is false and any other integer is true.
- `if (expression) { ... }` with optional `else { ... }`, and
  `while (expression) { ... }`. Braces are required.
- `print(expression);` prints an integer; `print("text");` prints a literal
  printable-ASCII string. Each print ends with a newline. String escapes and
  string variables are not supported.
- `//` comments extend to the end of the line.

There are at most 32 variables, a shared script-wide namespace, 16 levels of
interpreter nesting, and a budget of 10,000 parser/execution steps. A variable
must be declared before use and a duplicate declaration is an error. Because
names are script-wide, do not place a declaration in a loop that runs more than
once. Budget exhaustion stops runaway loops; it is not a time limit. There are
no functions, arrays, user input, `fork`, `exit`, `wait`, or process APIs yet.

## Script files and PATH

`run <path>` loads at most 4096 bytes from the read-only filesystem and runs
those exact bytes. A name containing `/` is resolved as an absolute path or
relative to the current directory. For a bare name, `run` searches PATH from
left to right. There is no implicit current-directory search; use `./name`.
Inaccessible and missing paths both report `script unavailable`; actual I/O or
size failures are reported separately. Existing filesystem access checks apply.

```text
path
path /home/usera/programs:/some/other/directory
run demo.rps
run ./demo.rps
```

PATH is currently **per shell session** for the one demo identity, initially
`/home/usera/programs`. Directories must be absolute, separated by colons, with
no empty entries; the full value is limited to 127 bytes. A bad value leaves the
old one intact. The PATH setting is not persistent and is not yet a process
environment or an executable search path; there are no login accounts or
executable loader. Access checks still apply to every searched directory and
file, so adding a path does not grant permission.

The guest filesystem cannot create or edit script files yet. An existing image
is never reformatted or populated automatically: use interactive mode now, or
run an already-present readable text file. File writes and a safe explicit host
import mechanism are future work.

## Validation

From `20_timer_callbacks`:

```powershell
.\run-qemu.ps1 -PrepareOnly
python -B .\tools\cpu_boot_smoke.py
python -B .\tools\script_boot_smoke.py
New-Item -ItemType Directory -Force .\target\script-tests | Out-Null
Remove-Item .\target\script-tests\script-tests.exe -ErrorAction SilentlyContinue
Remove-Item .\target\script-tests\script-tests.pdb -ErrorAction SilentlyContinue
rustc --edition=2021 --test .\tools\script_tests.rs -o .\target\script-tests\script-tests.exe
.\target\script-tests\script-tests.exe
```

The file-loading smoke test creates and removes a disposable disk copy; it checks
that the persistent disk remains unchanged. The QEMU build disables generated
jump tables because those would otherwise land in supervisor-only rodata; user
code must not be built with alternate flags without revalidating its ring-3
code and data references.
