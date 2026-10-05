"""Aider projection tests without installing Aider or contacting a provider."""
import contextlib
import importlib.util
import io
import os
from pathlib import Path
import sys
from types import ModuleType, SimpleNamespace
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location(
    "anvil_aider", Path(__file__).resolve().parents[1] / "src" / "ai_inference_aider.py"
)
HELPER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(HELPER)


class AiderProjectionTests(unittest.TestCase):
    def test_malformed_entries_do_not_break_valid_following_entries_or_leak(self):
        models = ModuleType("aider.models")
        models.MODEL_ALIASES = {}

        class Model:
            def __init__(self, name):
                self.reasoning_tag = None

            def send_completion(self, *args, **kwargs):
                message = SimpleNamespace(content="test commit", tool_calls=None, function_call=None)
                return None, SimpleNamespace(choices=[SimpleNamespace(message=message)])

        models.Model = Model
        onboarding = ModuleType("aider.onboarding")
        onboarding.try_to_select_default_model = lambda: "test-model"
        reasoning = ModuleType("aider.reasoning_tags")
        reasoning.remove_reasoning_content = lambda text, tag: text
        modules = {"aider.models": models, "aider.onboarding": onboarding, "aider.reasoning_tags": reasoning}
        output = io.StringIO()
        config = {
            "api-key": ["fake-secret-without-separator", None, " = value", "provider = fake-valid-key"],
            "alias": ["malformed", {}, " : value", "fast: provider/model:version"],
        }
        with patch.dict(sys.modules, modules), patch.dict(os.environ):
            with contextlib.redirect_stdout(output), contextlib.redirect_stderr(output):
                self.assertEqual(HELPER.generate(config, "test diff"), "test commit")
            self.assertEqual(os.environ["PROVIDER_API_KEY"], "fake-valid-key")
        self.assertEqual(models.MODEL_ALIASES, {"fast": "provider/model:version"})
        self.assertEqual(output.getvalue(), "")


if __name__ == "__main__":
    unittest.main()
