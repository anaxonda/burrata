"""Offline regression for controller failure preservation during PCM assessment."""
import copy
import unittest
from native_runtime_capture import incident_cases_passed


class IncidentResultGate(unittest.TestCase):
    def test_nonempty_completed_controller_and_pcm_cases_pass(self):
        self.assertTrue(incident_cases_passed({'passed': True, 'cases': [{'passed': True}]}))

    def test_empty_cases_cannot_replace_failed_controller_with_vacuous_success(self):
        for controller_passed in [False, True]:
            runtime = {'passed': controller_passed, 'cases': [],
                       'error': 'No qualified current-owner tempo estimate'}
            saved = copy.deepcopy(runtime)
            self.assertFalse(incident_cases_passed(runtime))
            self.assertEqual(runtime, saved)
        self.assertFalse(incident_cases_passed({'passed': True, 'cases': []}))

    def test_pcm_success_does_not_override_controller_failure_or_error(self):
        for runtime in [
            {'passed': False, 'cases': [{'passed': True}]},
            {'passed': True, 'error': 'fixture failure', 'cases': [{'passed': True}]},
            {'passed': True, 'cases': [{'passed': False}]},
            {'passed': True, 'cases': [{}]},
        ]:
            self.assertFalse(incident_cases_passed(runtime))


if __name__ == '__main__':
    unittest.main()
