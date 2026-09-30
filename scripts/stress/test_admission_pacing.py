"""Admission cadence includes work without removing its cooperative yield."""
import asyncio
from pathlib import Path
import tempfile
import unittest
from unittest.mock import AsyncMock, Mock, patch

from controller import Controller
from runtime import Journal, Quota


class AdmissionPacingTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        asyncio.get_running_loop().set_debug(False)
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.now = 1000.0
        self.monotonic = 0.0
        self.c = Controller.__new__(Controller)
        self.c.state = Path(self.tmp.name)
        (self.c.state / 'artifacts').mkdir()
        (self.c.state / 'reports').mkdir()
        self.c.j = Journal(self.c.state / 'journal.sqlite', lambda: self.now)
        self.addCleanup(self.c.j.db.close)
        self.c.j.set('status', 'running')
        self.c.j.set('setup_start', 1000)
        self.c.j.set('end', 10000)
        self.c.j.set('last_summary_at', 1000)
        self.c.plan = {'phases': []}
        self.c.events = []
        self.c.config = {}
        self.c.fresh = 1000
        self.c.closed = False
        self.c.sync = AsyncMock()
        self.c.accounting = Mock()
        self.c.report = Mock(return_value={})
        self.c.monitor = AsyncMock()
        self.c.reconcile = AsyncMock()
        self.starts, self.sleeps, self.tasks = [], [], []

    def advance(self, seconds):
        self.monotonic += seconds
        self.now += seconds

    async def run_ticks(self, durations, *, error=None, cancel=False, wall_jump=0):
        async def admission():
            self.starts.append(self.monotonic)
            self.advance(durations[len(self.starts)-1])
            self.now += wall_jump
            if error:
                raise error

        async def sleep(delay):
            self.sleeps.append(delay)
            self.advance(delay)
            if cancel:
                raise asyncio.CancelledError()
            if len(self.sleeps) == len(durations):
                self.c.j.set('status', 'rehearsal_complete')

        self.c.admission_tick = AsyncMock(side_effect=admission)
        create = asyncio.create_task
        def background(coroutine):
            task = create(coroutine)
            self.tasks.append(task)
            return task

        with (patch('controller.time.time', lambda: self.now),
              patch('controller.time.monotonic', lambda: self.monotonic),
              patch('controller.asyncio.sleep', side_effect=sleep),
              patch('controller.asyncio.create_task', side_effect=background)):
            try:
                await self.c.run()
            finally:
                await asyncio.gather(*self.tasks, return_exceptions=True)

    async def test_fast_work_keeps_nominal_one_second_cadence(self):
        await self.run_ticks([.25, .25, .25])
        self.assertEqual(self.starts, [0, 1, 2])
        self.assertEqual(self.sleeps, [.75, .75, .75])
        self.assertEqual(self.c.j.get('end'), 10000)

    async def test_slow_work_retains_only_bounded_yield(self):
        await self.run_ticks([1.4, 1.4])
        self.assertAlmostEqual(self.starts[1], 1.6)
        self.assertEqual(self.sleeps, [.2, .2])
        self.assertTrue(all(delay > 0 for delay in self.sleeps))

    async def test_idle_work_still_sleeps_a_full_second(self):
        await self.run_ticks([0, 0])
        self.assertEqual(self.starts, [0, 1])
        self.assertEqual(self.sleeps, [1, 1])

    async def test_wall_clock_jump_does_not_change_pacing(self):
        await self.run_ticks([.25, .25], wall_jump=-100)
        self.assertEqual(self.starts, [0, 1])
        self.assertEqual(self.sleeps, [.75, .75])

    async def test_summary_work_is_part_of_the_iteration(self):
        self.c.j.set('last_summary_at', -22000)
        def report():
            self.advance(.2)
            return {}
        self.c.report.side_effect = report
        await self.run_ticks([.3])
        self.assertAlmostEqual(self.sleeps[0], .3)
        self.assertEqual(len(list((self.c.state / 'reports').iterdir())), 1)

    async def test_quota_keeps_single_attempt_and_cooperative_yield(self):
        await self.run_ticks([1.2], error=Quota('rate budget'))
        self.assertEqual(self.sleeps, [.2])
        self.c.admission_tick.assert_awaited_once()
        events = self.c.j.db.execute("SELECT COUNT(*) FROM events WHERE state='admission_quota'").fetchone()[0]
        self.assertEqual(events, 1)
        self.assertEqual(self.c.j.rows(), [])

    async def test_cancellation_during_yield_closes_background_tasks(self):
        with self.assertRaises(asyncio.CancelledError):
            await self.run_ticks([1.2], cancel=True)
        self.assertEqual(self.sleeps, [.2])
        self.assertTrue(self.c.closed)
        self.assertEqual(len(self.tasks), 2)
        self.assertTrue(all(task.done() for task in self.tasks))
        self.c.admission_tick.assert_awaited_once()

    async def test_held_and_draining_remain_idle_and_stop_at_existing_bound(self):
        for status in ('held', 'draining'):
            with self.subTest(status=status):
                self.c.closed = False
                self.now = 1000
                self.monotonic = 0
                self.c.j.set('status', status)
                self.c.j.set('stopped_at', 1000)
                self.c.admission_tick = AsyncMock(side_effect=AssertionError('admission after hold'))
                waits = []
                async def sleep(delay):
                    waits.append(delay)
                    self.advance(delay+900)  # Simulate scheduling delay, no real waiting.
                with (patch('controller.time.time', lambda: self.now),
                      patch('controller.time.monotonic', lambda: self.monotonic),
                      patch('controller.asyncio.sleep', side_effect=sleep)):
                    await self.c.run()
                self.assertEqual(waits, [1, 1])
                self.c.admission_tick.assert_not_awaited()
                self.assertEqual(self.c.j.get('stopped_at'), 1000)
                self.assertEqual(self.c.j.get('end'), 10000)
                self.assertEqual(self.c.j.get('status'), 'incomplete')

if __name__ == '__main__':
    unittest.main()
