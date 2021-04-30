// This snapshot tests accessing various containers, dereferencing pointers.

[[block]]
struct Bar {
    data: [[stride(4)]] array<u32>;
};

[[group(0), binding(0)]]
var<storage> bar: [[access(read_write)]] Bar;

[[stage(vertex)]]
fn foo([[builtin(vertex_index)]] vi: u32) -> [[builtin(position)]] vec4<f32> {
	bar.data[0] = 0u; // Comment this out!
	
	let array = array<i32, 5>(1, 2, 3, 4, 5);
	let value = array[vi];
	return vec4<f32>(vec4<i32>(value));
}
