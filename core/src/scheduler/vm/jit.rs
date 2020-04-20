// Copyright (C) 2019-2020  Pierre Krieger
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

// TODO: everything here is unsafe and prototipal

use super::{ExecOutcome, NewErr, RunErr, StartErr};
use crate::{WasmValue, module::Module, primitives::Signature};

use alloc::{
    boxed::Box,
    rc::Rc,
    vec::Vec,
};
use core::{cell::RefCell, convert::TryFrom as _, fmt};

mod coroutine;

pub struct Jit<T> {
    main_thread: coroutine::Coroutine<Box<dyn FnOnce() -> Result<Option<wasmtime::Val>, wasmtime::Trap>>, Interrupt, Option<WasmValue>>,

    memory: Option<wasmtime::Memory>,
    indirect_table: Option<wasmtime::Table>,

    /// We only support one thread. That's its user data.
    thread_user_data: Option<T>,
}

struct Interrupt {
    function_index: usize,
    parameters: Vec<WasmValue>,
}

/// Access to a thread within the virtual machine.
pub struct Thread<'a, T> {
    /// Reference to the parent object.
    vm: &'a mut Jit<T>,
}

impl<T> Jit<T> {
    /// Creates a new process state machine from the given module.
    ///
    /// The closure is called for each import that the module has. It must assign a number to each
    /// import, or return an error if the import can't be resolved. When the VM calls one of these
    /// functions, this number will be returned back in order for the user to know how to handle
    /// the call.
    ///
    /// A single main thread (whose user data is passed by parameter) is automatically created and
    /// is paused at the start of the "_start" function of the module.
    pub fn new(
        module: &Module,
        main_thread_user_data: T,
        mut symbols: impl FnMut(&str, &str, &Signature) -> Result<usize, ()>,
    ) -> Result<Self, NewErr> {
        let engine = wasmtime::Engine::new(&Default::default());
        let store = wasmtime::Store::new(&engine);
        let module = wasmtime::Module::from_binary(&store, module.as_ref()).unwrap();

        let builder = coroutine::CoroutineBuilder::new();

        let mut imports = Vec::new();

        for import in module.imports() {
            match import.ty() {
                wasmtime::ExternType::Func(f) => {
                    // TODO: don't panic if not found
                    let function_index = symbols(import.module(), import.name(), &From::from(f)).unwrap();
                    let interrupter = builder.interrupter();
                    imports.push(wasmtime::Extern::Func(wasmtime::Func::new(&store, f.clone(), move |_, params, ret_val| {
                        let returned = interrupter.interrupt(Interrupt {
                            function_index,
                            parameters: params.iter().cloned().map(From::from).collect(),
                        });
                        if let Some(returned) = returned {
                            assert_eq!(ret_val.len(), 1);
                            ret_val[0] = From::from(returned);
                        } else {
                            assert!(ret_val.is_empty());
                        }
                        Ok(())
                    })));
                }
                wasmtime::ExternType::Global(_) => unimplemented!(),
                wasmtime::ExternType::Table(_) => unimplemented!(),
                wasmtime::ExternType::Memory(_) => unimplemented!(),
            };
        }

        let memory = Rc::new(RefCell::new(None));
        let indirect_table = Rc::new(RefCell::new(None));

        let mut main_thread = {
            let memory = memory.clone();
            let indirect_table = indirect_table.clone();
            let interrupter = builder.interrupter();
            builder.build(Box::new(move || {
                // TODO: don't unwrap
                let instance = wasmtime::Instance::new(&module, &imports).unwrap();

                if let Some(mem) = instance.get_export("memory") {
                    if let Some(mem) = mem.memory() {
                        *memory.borrow_mut() = Some(mem.clone());
                    }
                }
                if let Some(tbl) = instance.get_export("__indirect_function_table") {
                    if let Some(tbl) = tbl.table() {
                        *indirect_table.borrow_mut() = Some(tbl.clone());
                    }
                }

                // Try to start executing `_start`.
                let start_function = if let Some(f) = instance.get_export("_start") {
                    if let Some(f) = f.func() {
                        f.clone()
                    } else {
                        unimplemented!() // TODO: return Err(NewErr::StartIsntAFunction);
                    }
                } else {
                    unimplemented!() // TODO: return Err(NewErr::StartNotFound);
                };

                // TODO: dummy Interrupt
                let _reinjected: Option<WasmValue> = interrupter.interrupt(Interrupt {
                    function_index: 0,
                    parameters: Vec::new(),
                });
                assert!(_reinjected.is_none());

                let result = start_function.call(&[])?;
                assert!(result.len() == 0 || result.len() == 1); // TODO: I don't know what multiple results means
                if result.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(result[0].clone()))   // TODO: don't clone
                }
            }) as Box<_>)
        };

        let _ = main_thread.run(None);

        let memory = memory.borrow_mut().clone();
        let indirect_table = indirect_table.borrow_mut().clone();

        Ok(Jit {
            memory,
            indirect_table,
            main_thread,
            thread_user_data: Some(main_thread_user_data),
        })
    }

    /// Returns true if the state machine is in a poisoned state and cannot run anymore.
    pub fn is_poisoned(&self) -> bool {
        self.main_thread.is_finished()
    }

    pub fn start_thread_by_id(
        &mut self,
        _: u32,
        _: impl IntoIterator<Item = WasmValue>,
        _: T,
    ) -> Result<Thread<T>, StartErr> {
        unimplemented!()
    }

    /// Returns the number of threads that are running.
    pub fn num_threads(&self) -> usize {
        1
    }

    pub fn thread(&mut self, index: usize) -> Option<Thread<T>> {
        if index == 0 && !self.is_poisoned() {
            Some(Thread { vm: self })
        } else {
            None
        }
    }

    pub fn into_user_datas(self) -> impl ExactSizeIterator<Item = T> {
        self.thread_user_data.into_iter()
    }

    /// Copies the given memory range into a `Vec<u8>`.
    ///
    /// Returns an error if the range is invalid or out of range.
    pub fn read_memory(&self, offset: u32, size: u32) -> Result<Vec<u8>, ()> {
        let mem = match self.memory.as_ref() {
            Some(m) => m,
            None => return Err(()),
        };

        let start = usize::try_from(offset).map_err(|_| ())?;
        let end = start.checked_add(usize::try_from(size).map_err(|_| ())?).ok_or(())?;

        unsafe {
            Ok(mem.data_unchecked()[start..end].to_vec())
        }
    }

    /// Write the data at the given memory location.
    ///
    /// Returns an error if the range is invalid or out of range.
    pub fn write_memory(&mut self, offset: u32, value: &[u8]) -> Result<(), ()> {
        let mem = match self.memory.as_ref() {
            Some(m) => m,
            None => return Err(()),
        };

        let start = usize::try_from(offset).map_err(|_| ())?;
        let end = start.checked_add(value.len()).ok_or(())?;

        unsafe {
            mem.data_unchecked_mut()[start..end].copy_from_slice(value);
        }

        Ok(())
    }
}

unsafe impl<T: Send> Send for Jit<T> {}

impl<T> fmt::Debug for Jit<T>
where
    T: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_tuple("Jit").finish()
    }
}

impl<'a, T> Thread<'a, T> {
    /// Starts or continues execution of this thread.
    ///
    /// If this is the first call you call [`run`](Thread::run) for this thread, then you must pass
    /// a value of `None`.
    /// If, however, you call this function after a previous call to [`run`](Thread::run) that was
    /// interrupted by an external function call, then you must pass back the outcome of that call.
    pub fn run(mut self, value: Option<WasmValue>) -> Result<ExecOutcome<'a, T>, RunErr> {
        if self.vm.main_thread.is_finished() {
            return Err(RunErr::Poisoned)
        }

        // TODO: check value type

        match self.vm.main_thread.run(Some(value.map(From::from))) {
            coroutine::RunOut::Finished(Err(err)) => {
                Ok(ExecOutcome::Errored {
                    thread: From::from(self),
                    error: unimplemented!(), // TODO: err,
                })
            }
            coroutine::RunOut::Finished(Ok(val)) => {
                Ok(ExecOutcome::ThreadFinished {
                    thread_index: 0,
                    return_value: unimplemented!(), // TODO: Ok(val),
                    user_data: self.vm.thread_user_data.take().unwrap(),
                })
            }
            coroutine::RunOut::Interrupted(int) => {
                Ok(ExecOutcome::Interrupted {
                    thread: From::from(self),
                    id: int.function_index,
                    params: int.parameters,
                })
            }
        }
    }

    /// Returns the index of the thread, so that you can retreive the thread later by calling
    /// [`Jit::thread`].
    ///
    /// Keep in mind that when a thread finishes, all the indices above its index shift by one.
    pub fn index(&self) -> usize {
        0
    }

    /// Returns the user data associated to that thread.
    pub fn user_data(&mut self) -> &mut T {
        self.vm.thread_user_data.as_mut().unwrap()
    }

    /// Turns this thread into the user data associated to it.
    pub fn into_user_data(self) -> &'a mut T {
        self.vm.thread_user_data.as_mut().unwrap()
    }
}

impl<'a, T> fmt::Debug for Thread<'a, T>
where
    T: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_tuple("Thread")
            .field(&self.vm.thread_user_data)
            .finish()
    }
}