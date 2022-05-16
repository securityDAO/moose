import argparse
import logging

import numpy as np
from absl.testing import absltest
from absl.testing import parameterized

import pymoose as pm
from pymoose.computation import types as ty
from pymoose.logger import get_logger
from pymoose.testing import LocalMooseRuntime


class ReducemaxLogicExample(parameterized.TestCase):
    def _setup_comp(self):
        alice = pm.host_placement(name="alice")
        bob = pm.host_placement(name="bob")
        carole = pm.host_placement(name="carole")
        rep = pm.replicated_placement(name="rep", players=[alice, bob, carole])

        @pm.computation
        def my_comp(x_uri: pm.Argument(placement=bob, vtype=ty.StringType())):
            with bob:
                x = pm.load(x_uri, dtype=pm.float64)
                x_fixed = pm.cast(x, dtype=pm.fixed(8, 27))
                x0 = pm.index_axis(x_fixed, axis=2, index=0)
                x1 = pm.index_axis(x_fixed, axis=2, index=1)
                x2 = pm.index_axis(x_fixed, axis=2, index=2)

            with rep:
                x_max = pm.maximum([x0, x1, x2])

            with bob:
                x_max_host = pm.cast(x_max, dtype=pm.float64)
                res = pm.save("reduce_max", x_max_host)

            return res

        return my_comp

    @parameterized.parameters(
        ([[[1, 2, 3], [4, 5, 6]], [[7, 8, 9], [10, 11, 12]]],),
        ([[[1, 10, 1], [4, 100, 32]], [[123, 521, 132], [312, 421, 321]]],),
    )
    def test_example_execute(self, x):
        comp = self._setup_comp()
        traced_less_comp = pm.trace(comp)

        x_arg = np.array(x, dtype=np.float64)

        storage = {
            "alice": {},
            "carole": {},
            "bob": {"x_arg": x_arg},
        }

        runtime = LocalMooseRuntime(storage_mapping=storage)
        _ = runtime.evaluate_computation(
            computation=traced_less_comp,
            role_assignment={"alice": "alice", "bob": "bob", "carole": "carole"},
            arguments={"x_uri": "x_arg"},
        )

        x0 = runtime.read_value_from_storage("bob", "reduce_max")

        np.testing.assert_almost_equal(x0, x_arg.max(axis=2))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="comparison example")
    parser.add_argument("--verbose", action="store_true")
    args = parser.parse_args()

    if args.verbose:
        get_logger().setLevel(level=logging.DEBUG)

    absltest.main()
