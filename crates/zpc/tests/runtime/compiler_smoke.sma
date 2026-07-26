#include <amxmodx>

new const EXPECTED_TEXT[] = "20:ok"
new g_values[] = { 2, 4, 6, 8 }
new g_phase

public plugin_precache()
{
	g_phase = 1
}

public plugin_init()
{
	register_plugin("zpc runtime smoke", "1.0", "zplint")

	if (g_phase != 1)
		set_fail_state("zpc public/forward mismatch")

	new sum
	for (new i = 0; i < sizeof g_values; i++)
		sum += g_values[i]

	if (sum != 20 || factorial(5) != 120)
		set_fail_state("zpc arithmetic/control-flow mismatch")

	new matrix[2][3] = {
		{ 1, 2, 3 },
		{ 4, 5, 6 }
	}
	if (matrix[1][2] != 6)
		set_fail_state("zpc multidimensional-array mismatch")

	new Float:ratio = 7.5
	if (floatround(ratio * 2.0) != 15)
		set_fail_state("zpc float/user-operator mismatch")

	new byref = 10
	increment(byref)
	if (byref != 11)
		set_fail_state("zpc by-reference mismatch")

	new actual[16]
	formatex(actual, charsmax(actual), "%d:%s", sum, "ok")
	if (!equal(actual, EXPECTED_TEXT))
		set_fail_state("zpc string/native mismatch")

	switch (sum)
	{
		case 20: set_task(0.1, "runtime_async")
		default: set_fail_state("zpc switch mismatch")
	}
}

public runtime_async()
{
	if (g_phase != 1)
		set_fail_state("zpc global-state mismatch")

	server_print("ZPLINT_RUNTIME_PASS")
}

factorial(value)
{
	if (value <= 1)
		return 1

	return value * factorial(value - 1)
}

increment(&value)
{
	value++
}
