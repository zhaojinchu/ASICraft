from harness.design import all_vectors, sampled_vectors

MODULE = {"ports": {"a": {"direction": "input", "bits": [2, 3, 4]}, "b": {"direction": "input", "bits": [5, 6, 7]},
                    "y": {"direction": "output", "bits": [8]}}}


def test_all_vectors_covers_every_combination():
    vectors = all_vectors(MODULE)
    assert len(vectors) == 64
    assert len({tuple(sorted(v.items())) for v in vectors}) == 64


def test_sampling_keeps_small_designs_whole():
    assert sampled_vectors(MODULE, 64) == all_vectors(MODULE)


def test_sampling_is_fixed_and_ends_with_everything_on():
    sample = sampled_vectors(MODULE, 32)
    assert len(sample) == 32
    assert sample == sampled_vectors(MODULE, 32)
    assert all(value == 1 for value in sample[-1].values())
    assert all(value == 0 for value in sample[0].values())
    assert len({tuple(sorted(v.items())) for v in sample}) == 32


def test_plots_of_large_fixtures_do_not_overlap():
    from harness.runner import layout
    from harness.trace import Fixture

    def fixture(name, sx, sz):
        return Fixture(name=name, doc="", size=(sx, 4, sz), placement="strict", blocks={}, events=[], ticks=1, expect=[])

    plots = layout([fixture("small", 10, 10), fixture("wide", 365, 250), fixture("tall", 25, 70)])
    boxes = [p.placement.world_box() for p in plots]
    for i, a in enumerate(boxes):
        for b in boxes[i + 1:]:
            apart = a[1][0] < b[0][0] or b[1][0] < a[0][0] or a[1][2] < b[0][2] or b[1][2] < a[0][2]
            assert apart, (a, b)
    for plot, box in zip(plots, boxes):
        if plot.variant == "straddle":
            (x0, _, z0), (x1, _, z1) = box
            assert x0 // 16 != x1 // 16 and z0 // 16 != z1 // 16, box
        if plot.variant == "aligned":
            assert box[0][0] % 16 == 0 and box[0][2] % 16 == 0
