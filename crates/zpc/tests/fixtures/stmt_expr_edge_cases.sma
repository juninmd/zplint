/* Fixture: statements and expressions, including the Pawn-only forms.
 */
#include <amxmodx>

stock control_flow(id)
{
	new total = 0

	if (id > 0)
		total += 1
	else if (id < 0)
		total -= 1
	else
		total = 0

	while (total < 10)
		total++

	do
	{
		total--
	}
	while (total > 5)

	for (new i = 0; i < 32; i++)
	{
		if (i == 7)
			continue
		if (i == 20)
			break
		total += i
	}

	// Braceless single-statement bodies are legal and common in legacy plugins.
	for (new j = 0; j < 3; j++) total += j

	switch (total)
	{
		case 0: return 0
		case 1, 2, 3: return 1
		case 10 .. 20: return 2
		default: return -1
	}

	return total
}

stock expressions(a, b)
{
	new x

	// Precedence ladder.
	x = a + b * 2 - 1
	x = (a | b) & (a ^ b)
	x = a << 2 >> 1
	x = a >>> 3
	x = a && b || !a
	x = a > b ? a : b

	// Assignment operators.
	x += a
	x -= a
	x *= a
	x /= a
	x %= a
	x <<= 1
	x >>= 1
	x >>>= 1
	x &= a
	x |= a
	x ^= a

	// Unary and inc/dec.
	x = -a
	x = ~a
	x = !a
	x++
	++x
	x--
	--x

	// Compile-time operators.
	new arr[10]
	x = sizeof(arr)
	x = charsmax(arr)

	// Tag casts.
	new Float:f = float(a)
	x = floatround(f)
	x = _:f

	return x
}

stock labels_and_goto(id)
{
	new i = 0
loop:
	i++
	if (i < id)
		goto loop
	return i
}

public plugin_init()
{
	register_plugin("stmt/expr edge cases", "1.0", "zpc")
	control_flow(1)
	expressions(2, 3)
	labels_and_goto(4)
}
