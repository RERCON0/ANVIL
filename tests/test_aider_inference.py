"""Aider projection tests without installing Aider or contacting a provider."""
import contextlib
import importlib.util
import io
import json
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
            "api-key": [
                "fake-secret-without-separator", None, " = value", "provider = fake-valid-key",
                "invalid name=fake-key", "bad\x00name=fake-key", "кириллица=fake-key",
            ],
            "alias": ["malformed", {}, " : value", "fast: provider/model:version"],
        }
        with patch.dict(sys.modules, modules), patch.dict(os.environ):
            with contextlib.redirect_stdout(output), contextlib.redirect_stderr(output):
                self.assertEqual(HELPER.generate(config, "test diff"), "test commit")
            self.assertEqual(os.environ["PROVIDER_API_KEY"], "fake-valid-key")
        self.assertEqual(models.MODEL_ALIASES, {"fast": "provider/model:version"})
        self.assertEqual(output.getvalue(), "")

    @staticmethod
    def run_main(config, generate):
        # A pipe to the parent is written in the ANSI code page (cp1251 on a
        # Russian Windows), not in UTF-8; the isolated interpreter ignores
        # PYTHONUTF8 and PYTHONIOENCODING.
        stdout = io.TextIOWrapper(io.BytesIO(), encoding="cp1251", write_through=True)
        with patch.object(sys, "argv", ["inference.py", "--message", "diff"]), patch.object(
            sys, "stdout", stdout
        ), patch.object(HELPER, "generate", generate), patch.dict(
            os.environ, {"ANVIL_AIDER_CONFIG": json.dumps(config)}
        ):
            code = HELPER.main()
        return code, json.loads(stdout.buffer.getvalue().decode("utf-8"))

    def test_non_ascii_messages_survive_the_code_page_of_a_pipe(self):
        message = "feat: поддержка кириллицы \U0001f642 …"
        code, document = self.run_main({}, lambda config, prompt, model=None: message)
        self.assertEqual((code, document), (0, {"result": message}))

    def test_provider_errors_do_not_echo_the_credentials_we_were_given(self):
        config = {"openai-api-key": "sk-live-SECRETVALUE", "api-key": ["gemini=AIzaGEMINISECRET"]}

        def failing(config, prompt, model=None):
            raise RuntimeError("401 at https://x.example/v1?key=AIzaGEMINISECRET for Bearer sk-live-SECRETVALUE")

        code, document = self.run_main(config, failing)
        self.assertEqual(code, 1)
        self.assertEqual(document, {"error": "401 at https://x.example/v1?key=… for Bearer …"})


if __name__ == "__main__":
    unittest.main()
