# SPDX-License-Identifier: MIT
"""Deterministic UDP impairment worker error handling without child processes."""

import importlib.util
from pathlib import Path
import threading
import unittest
from unittest import mock


SPEC = importlib.util.spec_from_file_location(
    "smoke_call", Path(__file__).with_name("smoke_call.py")
)
SMOKE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(SMOKE)


class ImpairedLinkErrors(unittest.TestCase):
    def link(self):
        # Run the real worker synchronously with deterministic socket events.
        link = SMOKE.ImpairedLink.__new__(SMOKE.ImpairedLink)
        link.destinations = [("127.0.0.1", 10001), ("127.0.0.1", 10002)]
        link.mode = "loss-ipv4"
        link.sockets = [mock.Mock(), mock.Mock()]
        link.stopped = threading.Event()
        link.error = None
        link.dropped = 0
        link.thread = mock.Mock()
        link.thread.is_alive.return_value = False
        return link

    def test_transient_receive_errors_preserve_the_next_datagram(self):
        for error_type in (ConnectionRefusedError, ConnectionResetError):
            with self.subTest(error=error_type.__name__):
                link = self.link()
                source, destination = link.sockets
                packet = b"SNC1" + bytes([2]) + bytes(25)
                source.recvfrom.side_effect = [
                    error_type("peer port not open"),
                    (packet, link.destinations[0]),
                ]
                destination.sendto.side_effect = lambda *_: link.stopped.set()
                with mock.patch.object(
                    SMOKE.select,
                    "select",
                    side_effect=[([source], [], []), ([source], [], [])],
                ) as select_ready:
                    link.run()
                self.assertIsNone(link.error)
                self.assertEqual(select_ready.call_count, 2)
                self.assertEqual(source.recvfrom.call_count, 2)
                destination.sendto.assert_called_once_with(packet, link.destinations[1])
                self.assertEqual(link.dropped, 0)
                link.close()

    def test_unexpected_receive_error_is_reported_with_its_original_cause(self):
        link = self.link()
        source, destination = link.sockets
        error = PermissionError("unexpected socket failure")
        source.recvfrom.side_effect = error
        with mock.patch.object(SMOKE.select, "select", return_value=([source], [], [])):
            link.run()
        self.assertIs(link.error, error)
        destination.sendto.assert_not_called()
        with self.assertRaisesRegex(RuntimeError, "Impairment worker failed") as caught:
            link.close()
        self.assertIs(caught.exception.__cause__, error)
        link.thread.join.assert_called_once_with(timeout=2)
        for endpoint in link.sockets:
            endpoint.close.assert_called_once_with()


if __name__ == "__main__":
    unittest.main()
