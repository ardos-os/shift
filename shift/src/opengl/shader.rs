use std::ffi::CString;

use easydrm::gl;

use crate::opengl::binding::{BindingGuard, read_binding};
use crate::renderer::RendererError;

pub struct Shader {
	gl: gl::Gles2,
	program: u32,
}

impl Shader {
	pub fn new(gl: &gl::Gles2, vertex_src: &str, fragment_src: &str) -> Result<Self, RendererError> {
		let vertex = compile_shader(gl, gl::VERTEX_SHADER, vertex_src)?;
		let fragment = compile_shader(gl, gl::FRAGMENT_SHADER, fragment_src)?;
		let program = link_program(gl, vertex, fragment)?;
		gl!(gl, DeleteShader(vertex));
		gl!(gl, DeleteShader(fragment));
		Ok(Self {
			gl: gl.clone(),
			program,
		})
	}

	pub fn bind(&self) -> BindingGuard<'_, u32, impl Fn(&gl::Gles2, u32)> {
		BindingGuard::new(
			&self.gl,
			read_binding(&self.gl, gl::CURRENT_PROGRAM),
			|gl, program| {
				gl!(gl, UseProgram(program));
			},
			self.program,
		)
	}

	#[allow(dead_code)]
	pub fn attrib_location(&self, name: &str) -> i32 {
		let c = CString::new(name).unwrap();
		gl!(&self.gl, GetAttribLocation(self.program, c.as_ptr()))
	}

	pub fn uniform_location(&self, name: &str) -> i32 {
		let c = CString::new(name).unwrap();
		gl!(&self.gl, GetUniformLocation(self.program, c.as_ptr()))
	}
}

impl Drop for Shader {
	fn drop(&mut self) {
		if self.program != 0 {
			gl!(&self.gl, DeleteProgram(self.program));
		}
	}
}

fn compile_shader(gl: &gl::Gles2, kind: u32, source: &str) -> Result<u32, RendererError> {
	let shader = gl!(gl, CreateShader(kind));
	let c_str = CString::new(source).unwrap();
	let ptr = c_str.as_ptr();
	gl!(gl, ShaderSource(shader, 1, &ptr, std::ptr::null()));
	gl!(gl, CompileShader(shader));
	let mut status = 0;
	gl!(gl, GetShaderiv(shader, gl::COMPILE_STATUS, &mut status));
	if status == gl::TRUE as i32 {
		Ok(shader)
	} else {
		let mut len = 0;
		gl!(gl, GetShaderiv(shader, gl::INFO_LOG_LENGTH, &mut len));
		let mut buf = vec![0u8; len as usize];
		gl!(
			gl,
			GetShaderInfoLog(shader, len, std::ptr::null_mut(), buf.as_mut_ptr().cast())
		);
		let log = String::from_utf8_lossy(&buf).trim().to_string();
		gl!(gl, DeleteShader(shader));
		Err(RendererError::Shader(log))
	}
}

fn link_program(gl: &gl::Gles2, vertex: u32, fragment: u32) -> Result<u32, RendererError> {
	let program = gl!(gl, CreateProgram());
	gl!(gl, AttachShader(program, vertex));
	gl!(gl, AttachShader(program, fragment));
	gl!(gl, LinkProgram(program));
	let mut status = 0;
	gl!(gl, GetProgramiv(program, gl::LINK_STATUS, &mut status));
	if status == gl::TRUE as i32 {
		Ok(program)
	} else {
		let mut len = 0;
		gl!(gl, GetProgramiv(program, gl::INFO_LOG_LENGTH, &mut len));
		let mut buf = vec![0u8; len as usize];
		gl!(
			gl,
			GetProgramInfoLog(program, len, std::ptr::null_mut(), buf.as_mut_ptr().cast())
		);
		let log = String::from_utf8_lossy(&buf).trim().to_string();
		gl!(gl, DeleteProgram(program));
		Err(RendererError::Program(log))
	}
}
