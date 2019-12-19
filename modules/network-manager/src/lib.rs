// Copyright (C) 2019  Pierre Krieger
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

use futures::prelude::*;
use hashbrown::{HashMap, hash_map::Entry};
use std::{hash::Hash, net::SocketAddr};

mod interface;

pub struct NetworkManager<TIfId> {
    devices: HashMap<TIfId, interface::NetInterfaceState>,
}

pub enum NetworkManagerEvent {
    Foo,
}

pub struct TcpSocket<'a, TIfId> {
    inner: interface::TcpSocket<'a>,
    device_id: TIfId,
}

impl<TIfId> NetworkManager<TIfId>
where
    TIfId: Clone + Hash + PartialEq + Eq,
{
    pub fn new() -> Self {
        NetworkManager {
            devices: HashMap::new(),
        }
    }

    pub fn tcp_connect(&mut self, dest: &SocketAddr) -> TcpSocket<TIfId> {
        for (device_id, device) in self.devices.iter_mut() {
            if let Ok(socket) = device.tcp_connect(dest) {
                return TcpSocket {
                    inner: socket,
                    device_id: device_id.clone(),
                };
            }
        }

        panic!()        // TODO:
    }

    /// Registers an interface with the given ID. Returns an error if an interface with that ID
    /// already exists.
    pub fn register_interface(&mut self, id: TIfId, mac_address: [u8; 6]) -> Result<(), ()> {
        let entry = match self.devices.entry(id) {
            Entry::Occupied(_) => return Err(()),
            Entry::Vacant(e) => e,
        };

        let interface = interface::NetInterfaceStateBuilder::new()
            .with_mac_address(mac_address)
            .build();
        entry.insert(interface);
        Ok(())
    }

    pub fn unregister_interface(&mut self, id: &TIfId) {
        let device = self.devices.remove(id);
        // TODO:
    }

    /// Returns the next event generated by the [`NetworkManager`].
    pub async fn next_event(&mut self) -> NetworkManagerEvent {
        // TODO: optimize?
        let next_event = future::select_all(self.devices.iter().map(|(n, d)| Box::pin(d.next_event().map(|ev| (n, ev)))));
        match next_event.await.0 {
        }
    }
}

impl<'a, TIfId> TcpSocket<'a, TIfId> {
    /// Closes the socket.
    pub fn close(self) {
        //self.device.
    }
}