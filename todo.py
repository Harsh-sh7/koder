#!/usr/bin/env python3
"""Simple command-line Todo application."""

import json
import sys
from pathlib import Path
from typing import Optional

DATA_FILE = Path.home() / ".todos.json"


def load_todos() -> list[dict]:
    """Load todos from the JSON file."""
    if not DATA_FILE.exists():
        return []
    try:
        with open(DATA_FILE, "r") as f:
            return json.load(f)
    except (json.JSONDecodeError, IOError):
        return []


def save_todos(todos: list[dict]) -> None:
    """Save todos to the JSON file."""
    with open(DATA_FILE, "w") as f:
        json.dump(todos, f, indent=2)


def add_todo(text: str) -> dict:
    """Add a new todo item."""
    if not text or not text.strip():
        raise ValueError("Todo text cannot be empty")
    
    todos = load_todos()
    todo = {
        "id": max([t["id"] for t in todos], default=0) + 1,
        "text": text.strip(),
        "completed": False
    }
    todos.append(todo)
    save_todos(todos)
    return todo


def list_todos() -> list[dict]:
    """List all todos."""
    return load_todos()


def complete_todo(todo_id: int) -> Optional[dict]:
    """Mark a todo as completed."""
    todos = load_todos()
    for todo in todos:
        if todo["id"] == todo_id:
            todo["completed"] = True
            save_todos(todos)
            return todo
    return None


def delete_todo(todo_id: int) -> bool:
    """Delete a todo by ID."""
    todos = load_todos()
    for i, todo in enumerate(todos):
        if todo["id"] == todo_id:
            todos.pop(i)
            save_todos(todos)
            return True
    return False


def print_todos() -> None:
    """Print todos in a formatted way."""
    todos = list_todos()
    if not todos:
        print("No todos found.")
        return
    
    for todo in todos:
        status = "[x]" if todo["completed"] else "[ ]"
        print(f'{todo["id"]}. {status} {todo["text"]}')


def main() -> int:
    """Main entry point."""
    if len(sys.argv) < 2:
        print("Usage: todo.py <command> [arguments]")
        print("Commands: add <text>, list, complete <id>, delete <id>")
        return 1
    
    command = sys.argv[1].lower()
    
    try:
        if command == "add":
            if len(sys.argv) < 3:
                print("Usage: todo.py add <text>")
                return 1
            todo = add_todo(" ".join(sys.argv[2:]))
            print(f'Added todo #{todo["id"]}: {todo["text"]}')
        
        elif command == "list":
            print_todos()
        
        elif command == "complete":
            if len(sys.argv) < 3:
                print("Usage: todo.py complete <id>")
                return 1
            try:
                todo_id = int(sys.argv[2])
            except ValueError:
                print("Error: ID must be a number")
                return 1
            todo = complete_todo(todo_id)
            if todo:
                print(f'Marked todo #{todo_id} as completed')
            else:
                print(f"Todo #{todo_id} not found")
                return 1
        
        elif command == "delete":
            if len(sys.argv) < 3:
                print("Usage: todo.py delete <id>")
                return 1
            try:
                todo_id = int(sys.argv[2])
            except ValueError:
                print("Error: ID must be a number")
                return 1
            if delete_todo(todo_id):
                print(f'Deleted todo #{todo_id}')
            else:
                print(f"Todo #{todo_id} not found")
                return 1
        
        else:
            print(f"Unknown command: {command}")
            print("Commands: add <text>, list, complete <id>, delete <id>")
            return 1
        
        return 0
    
    except ValueError as e:
        print(f"Error: {e}")
        return 1


if __name__ == "__main__":
    sys.exit(main())