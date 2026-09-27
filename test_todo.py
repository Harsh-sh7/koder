#!/usr/bin/env python3
"""Tests for the Todo application."""

import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

# Import the todo module
import todo


class TestTodoApp(unittest.TestCase):
    """Test cases for the todo application."""

    def setUp(self):
        """Set up test fixtures."""
        # Create a temporary file for testing
        self.temp_file = tempfile.NamedTemporaryFile(mode='w', suffix='.json', delete=False)
        self.temp_file.close()
        self.original_data_file = todo.DATA_FILE
        todo.DATA_FILE = Path(self.temp_file.name)
        # Start with empty todos
        with open(self.temp_file.name, 'w') as f:
            json.dump([], f)

    def tearDown(self):
        """Clean up test fixtures."""
        todo.DATA_FILE = self.original_data_file
        os.unlink(self.temp_file.name)

    def test_add_todo(self):
        """Test adding a new todo."""
        todo_item = todo.add_todo("Buy groceries")
        self.assertEqual(todo_item["id"], 1)
        self.assertEqual(todo_item["text"], "Buy groceries")
        self.assertFalse(todo_item["completed"])

    def test_add_multiple_todos(self):
        """Test adding multiple todos."""
        todo1 = todo.add_todo("First todo")
        todo2 = todo.add_todo("Second todo")
        self.assertEqual(todo1["id"], 1)
        self.assertEqual(todo2["id"], 2)

    def test_add_empty_todo_raises_error(self):
        """Test that adding an empty todo raises ValueError."""
        with self.assertRaises(ValueError):
            todo.add_todo("")
        with self.assertRaises(ValueError):
            todo.add_todo("   ")

    def test_list_todos(self):
        """Test listing todos."""
        todo.add_todo("Task 1")
        todo.add_todo("Task 2")
        todos = todo.list_todos()
        self.assertEqual(len(todos), 2)

    def test_list_empty_todos(self):
        """Test listing when there are no todos."""
        todos = todo.list_todos()
        self.assertEqual(len(todos), 0)

    def test_complete_todo(self):
        """Test marking a todo as completed."""
        added = todo.add_todo("Task to complete")
        completed = todo.complete_todo(added["id"])
        self.assertTrue(completed["completed"])

    def test_complete_nonexistent_todo(self):
        """Test completing a non-existent todo."""
        result = todo.complete_todo(999)
        self.assertIsNone(result)

    def test_delete_todo(self):
        """Test deleting a todo."""
        added = todo.add_todo("Task to delete")
        deleted = todo.delete_todo(added["id"])
        self.assertTrue(deleted)
        todos = todo.list_todos()
        self.assertEqual(len(todos), 0)

    def test_delete_nonexistent_todo(self):
        """Test deleting a non-existent todo."""
        deleted = todo.delete_todo(999)
        self.assertFalse(deleted)

    def test_persistence(self):
        """Test that todos persist across operations."""
        todo.add_todo("Persistent task")
        todos1 = todo.list_todos()
        self.assertEqual(len(todos1), 1)
        
        # Close and reload
        todos2 = todo.load_todos()
        self.assertEqual(len(todos2), 1)
        self.assertEqual(todos2[0]["text"], "Persistent task")


class TestTodoCLI(unittest.TestCase):
    """Test cases for CLI commands."""

    def setUp(self):
        """Set up test fixtures."""
        self.temp_file = tempfile.NamedTemporaryFile(mode='w', suffix='.json', delete=False)
        self.temp_file.close()
        self.original_data_file = todo.DATA_FILE
        todo.DATA_FILE = Path(self.temp_file.name)
        with open(self.temp_file.name, 'w') as f:
            json.dump([], f)

    def tearDown(self):
        """Clean up test fixtures."""
        todo.DATA_FILE = self.original_data_file
        os.unlink(self.temp_file.name)

    def test_cli_add(self):
        """Test CLI add command."""
        with patch.object(sys, 'argv', ['todo.py', 'add', 'Test task']):
            result = todo.main()
            self.assertEqual(result, 0)

    def test_cli_list_empty(self):
        """Test CLI list command with no todos."""
        with patch.object(sys, 'argv', ['todo.py', 'list']):
            result = todo.main()
            self.assertEqual(result, 0)

    def test_cli_complete(self):
        """Test CLI complete command."""
        added = todo.add_todo("Task to complete")
        with patch.object(sys, 'argv', ['todo.py', 'complete', str(added["id"])]):
            result = todo.main()
            self.assertEqual(result, 0)

    def test_cli_delete(self):
        """Test CLI delete command."""
        added = todo.add_todo("Task to delete")
        with patch.object(sys, 'argv', ['todo.py', 'delete', str(added["id"])]):
            result = todo.main()
            self.assertEqual(result, 0)

    def test_cli_invalid_command(self):
        """Test CLI with invalid command."""
        with patch.object(sys, 'argv', ['todo.py', 'invalid']):
            result = todo.main()
            self.assertEqual(result, 1)


if __name__ == '__main__':
    unittest.main()