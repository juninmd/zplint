/* Fixture: declaration and type-system constructs the parser must handle.
 * Drawn from the shapes that actually appear in the amxmodx bundled includes.
 */
#include <amxmodx>

// Tagged enum: members carry the enum's tag.
enum CsArmorType
{
	CS_ARMOR_NONE = 0,
	CS_ARMOR_KEVLAR = 1,
	CS_ARMOR_VESTHELM = 2,
};

// Enum with a step expression - each member is the previous shifted left.
enum SetTaskFlags (<<= 1)
{
	SetTask_Once = 0,
	SetTask_RepeatTimes = 1,
	SetTask_Repeat,
	SetTask_AfterMapStart,
	SetTask_BeforeMapChange
};

// Anonymous enum used as plain constants.
enum
{
	STATE_IDLE,
	STATE_RUNNING,
	STATE_DONE
};

// Enum whose members are array-sized struct-like field offsets.
enum PlayerData
{
	pd_name[32],
	pd_score,
	Float:pd_position[3]
};

// Variables in their many forms.
new g_simple
new g_initialised = 5
new Float:g_float = 1.5
new bool:g_flag = true
new g_array[10]
new g_matrix[4][8]
new g_inferred[] = {1, 2, 3, 4}
new g_string[] = "text"
new const g_readonly = 100
static g_file_local
stock g_maybe_unused
new g_data[PlayerData]

// Multi-dimensional with a fill range.
new g_filled[8] = {0, ...}

// Function declaration forms.
forward my_forward(id, Float:value)
native my_native(id, const message[], len)

// Return tag, default args, by-ref, const array arg, rest args.
stock Float:compute(id, Float:scale = 1.0, &result = 0, const label[] = "", ...)
{
	result = id
	return float(id) * scale + float(strlen(label))
}

public with_public_prefix(id)
{
	return id
}

static stock combined_modifiers(a)
{
	return a
}

// Operator overload.
stock CsArmorType:operator+(CsArmorType:a, CsArmorType:b)
{
	return CsArmorType:(_:a + _:b)
}

public plugin_init()
{
	register_plugin("decl edge cases", "1.0", "zpc")
}
