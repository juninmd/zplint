/* Fixture: lexical edge cases that a naive Pawn lexer gets wrong.
 * Every construct here is legal Pawn and must survive lexing unchanged.
 */
#include <amxmodx>

// The escape character is ^ by default, NOT backslash.
new g_msg[] = "line one^nline two^ttabbed"
new g_quote[] = "he said ^"hi^" loudly"
new g_hex[] = "byte ^x41; and octal ^65;"

// A backslash is an ORDINARY character in a default Pawn string.
new g_path[] = "addons\amxmodx\configs"

//* this is a line comment, not a block comment open
new g_after_tricky_comment = 1

// Operator greediness: these must lex as single tokens.
stock shift_ops(a, b)
{
	new x = a
	x >>>= b
	x <<= b
	x >>= b
	return x >>> b
}

// Char literals, including escaped ones.
new g_char = 'A'
new g_newline_char = '^n'
new g_quote_char = '^''

// Numeric forms.
new g_dec = 1234
new g_hex_num = 0xDEADBEEF
new g_bin = 0b1010
new g_neg = -42

// Rational literal (needs digits on both sides of the dot).
new Float:g_rational = 1.5
new Float:g_small = 0.001

// Multi-line string continuation using the control character.
new g_long[] = "first part ^
second part"

// Range and rest tokens.
stock range_and_rest(...)
{
	switch (numargs())
	{
		case 1 .. 5: return 1
		case 6, 7: return 2
		default: return 0
	}
	return 0
}

public plugin_init()
{
	register_plugin("lexer edge cases", "1.0", "zpc")
}
