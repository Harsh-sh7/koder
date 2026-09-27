# Simple Todo CLI Application

A simple command-line Todo application in Python that stores todos locally in a JSON file.

## Requirements

- Python 3.10 or higher
- No external dependencies required (uses only standard library)

## How to Run

### Run the application

```bash
python3 todo.py <command> [arguments]
```

### Commands

- **`add <text>`** - Add a new todo item
  ```bash
  python3 todo.py add "Buy groceries"
  python3 todo.py add "Finish the report"
  ```

- **`list`** - List all todos
  ```bash
  python3 todo.py list
  ```

- **`complete <id>`** - Mark a todo as completed
  ```bash
  python3 todo.py complete 1
  ```

- **`delete <id>`** - Delete a todo by ID
  ```bash
  python3 todo.py delete 1
  ```

## Examples

```bash
# Add some todos
$ python3 todo.py add "Buy groceries"
Added todo #1: Buy groceries

$ python3 todo.py add "Write documentation"
Added todo #2: Write documentation

# List todos
$ python3 todo.py list
1. [ ] Buy groceries
2. [ ] Write documentation

# Complete a todo
$ python3 todo.py complete 1
Marked todo #1 as completed

# List again
$ python3 todo.py list
1. [x] Buy groceries
2. [ ] Write documentation

# Delete a todo
$ python3 todo.py delete 2
Deleted todo #2
```

## Data Storage

Todos are stored in `~/.todos.json` (in your home directory) as JSON. This allows todos to persist between different runs of the application.

## Running Tests

Run the test suite using Python's built-in unittest framework:

```bash
python3 -m unittest test_todo.py -v
```

Or simply:

```bash
python3 test_todo.py
```

## Exit Codes

- `0` - Success
- `1` - Error (unknown command, missing arguments, invalid input, etc.)