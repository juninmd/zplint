/* Fixture: preprocessor behaviour that differs from C.
 * Pawn function-like macros use %1..%9, not named parameters.
 */
#include <amxmodx>

#define MAXPLAYERS 32
#define VERSION "1.0"

// Function-like macro with Pawn's positional parameters.
#define SQUARE(%1) ((%1) * (%1))
#define MAX_OF(%1,%2) ((%1) > (%2) ? (%1) : (%2))

// Macro referencing another macro.
#define DOUBLE_MAX (MAXPLAYERS * 2)

#if defined MAXPLAYERS
	#define HAS_MAX 1
#else
	#define HAS_MAX 0
#endif

#if MAXPLAYERS > 16
	#define BIG_SERVER
#elseif MAXPLAYERS > 8
	#define MEDIUM_SERVER
#else
	#define SMALL_SERVER
#endif

// Nested conditionals.
#if defined BIG_SERVER
	#if defined HAS_MAX
		#define CONFIG_OK
	#endif
#endif

#pragma semicolon 1
#pragma ctrlchar '\'

// After #pragma ctrlchar '\', the escape character is now backslash.
new g_escaped[] = "now \n is a newline and ^ is literal";

#pragma ctrlchar '^'

stock use_macros()
{
	new a = SQUARE(4);
	new b = MAX_OF(3, 7);
	new c = DOUBLE_MAX;
	// A macro name inside a string must NOT be substituted.
	new msg[] = "MAXPLAYERS should stay literal here";
	return a + b + c + msg[0];
}

public plugin_init()
{
	register_plugin("preproc edge cases", VERSION, "zpc");
}
