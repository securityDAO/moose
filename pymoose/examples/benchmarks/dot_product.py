import argparse
import logging

import numpy as np

import pymoose as pm
from pymoose.logger import get_logger

alice = pm.host_placement("alice")
bob = pm.host_placement("bob")
carole = pm.host_placement("carole")

rep = pm.replicated_placement(name="rep", players=[alice, bob, carole])


@pm.computation
def dot_product_comp(
    x_arg: pm.Argument(placement=alice, vtype=pm.TensorType(pm.float64)),
    y_arg: pm.Argument(placement=bob, vtype=pm.TensorType(pm.float64)),
):
    with alice:
        x = pm.cast(x_arg, dtype=pm.fixed(8, 27))

    with bob:
        y = pm.cast(y_arg, dtype=pm.fixed(8, 27))

    with rep:
        z = pm.dot(x, y)

    with carole:
        res = pm.cast(z, pm.float64)

    return res


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Example")
    parser.add_argument("--verbose", action="store_true")

    parser.add_argument(
        "--shape",
        dest="shape",
        type=int,
        nargs="+",
        default="1",
        help="shape used for dot products",
    )

    parser.add_argument(
        "--n",
        dest="n_iter",
        type=int,
        default="1",
        help="number of iterations for averaging the experiment",
    )
    args = parser.parse_args()
    shape = args.shape
    n_iter = args.n_iter

    if isinstance(shape, list) and len(shape) > 2:
        raise ValueError(
            f"Tensor shape expects at most a 2D tensor, found shape {shape}"
        )

    if args.verbose:
        get_logger().setLevel(level=logging.DEBUG)

    role_map = {
        alice: "localhost:50000",
        bob: "localhost:50001",
        carole: "localhost:50002",
    }

    runtime = pm.GrpcMooseRuntime(role_map)
    runtime.set_default()

    x = np.ones(shape, dtype=np.float64)
    y = np.ones(shape, dtype=np.float64)

    AVG_TIME = 0
    for _ in range(n_iter):
        outputs, timings = runtime.evaluate_computation(
            computation=dot_product_comp, arguments={"x_arg": x, "y_arg": y}
        )
        AVG_TIME += max(timings.values())

    print(f"On average all outputs are ready in {AVG_TIME / n_iter * 0.001} ms")
