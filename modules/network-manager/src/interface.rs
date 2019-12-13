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

use hashbrown::HashMap;
use smoltcp::iface::{EthernetInterface, EthernetInterfaceBuilder, NeighborCache, Routes};
use smoltcp::socket::{SocketHandle, SocketSet};
use smoltcp::{phy, time::Instant, wire::EthernetAddress};
use std::{
    collections::BTreeMap,
    fmt,
    mem,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

/// State machine encompassing an Ethernet interface and the sockets operating on it.
pub struct NetworkInterface {
    /// State of the Ethernet interface.
    ethernet: smoltcp::iface::EthernetInterface<'static, 'static, 'static, RawDevice>,

    /// Buffer of data to send out to the virtual Ethernet cable.
    /// Shared with the device within [`NetworkInterface::ethernet`].
    /// Note: this is a hack around the fact that the API of `smoltcp` wasn't really designed
    /// around being able to retreive the data of the interface.
    device_out_buffer: Arc<Mutex<Vec<u8>>>,

    /// Buffer of data received from the virtual Ethernet cable.
    /// Shared with the device within [`NetworkInterface::ethernet`].
    /// Note: this is a hack around the fact that the API of `smoltcp` wasn't really designed
    /// around being able to retreive the data of the interface.
    device_in_buffer: Arc<Mutex<Vec<u8>>>,

    /// Collection of all the active sockets that currently operate on this interface.
    sockets: smoltcp::socket::SocketSet<'static, 'static, 'static>,

    /// State of the sockets. Maintained in parallel with [`NetworkInterface`].
    sockets_state: HashMap<SocketId, SocketState>,

    /// Future that triggers the next time we should poll [`NetworkInterface::ethernet`].
    /// Must be set to `None` whenever we modify [`NetworkInterface::ethernet`] in such a way that
    /// it could produce an event.
    next_event_delay: Option<nametbd_time_interface::Delay>,
}

/// Prototype for a [`NetworkInterface`] under construction.
pub struct NetworkInterfaceBuilder {
    /// List of IP addresses that this interface will handle.
    ip_addresses: Vec<smoltcp::wire::IpCidr>,
}

/// Event generated by the [`NetworkInterface::next_event`] function.
#[derive(Debug)]
pub enum NetworkInterfaceEvent<'a> {
    /// Data to be sent out by the Ethernet cable.
    EthernetCableOut(Vec<u8>),
    /// A TCP/IP listener has received an incoming connection.
    TcpIncoming,
    /// A TCP/IP socket has connected to its target.
    TcpConnected(TcpSocket<'a>),
    TcpReadReady(TcpSocket<'a>),
    TcpWriteFinished(TcpSocket<'a>),
}

/// Active TCP socket within a [`NetworkInterface`].
pub struct TcpSocket<'a> {
    /// Reference to the interface.
    interface: &'a mut NetworkInterface,
    /// Identifier of that socket within [`NetworkInterface::sockets`].
    id: SocketId,
}

struct SocketState {
    connected: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    #[error("No route available for this destination")]
    NoRoute,
}

/// Opaque identifier of a socket within a [`NetworkInterface`].
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SocketId(smoltcp::socket::SocketHandle);

impl NetworkInterface {
    /// Innitializes a new TCP connection which tries to connect to the given
    /// [`SocketAddr`](std::net::SocketAddr).
    pub fn tcp_connect(&mut self, dest: &SocketAddr) -> Result<TcpSocket, ConnectError> {
        let mut socket = {
            let rx_buf = smoltcp::socket::TcpSocketBuffer::new(vec![0; 1024]);
            let tx_buf = smoltcp::socket::TcpSocketBuffer::new(vec![0; 1024]);
            smoltcp::socket::TcpSocket::new(rx_buf, tx_buf)
        };

        let socket = socket.connect(dest.clone(), dest.clone()).unwrap(); // TODO: bad source
        let id = SocketId(self.sockets.add(socket));
        self.sockets.insert(id, SocketState {
            connected: false,
        });
        self.next_event_delay = None;

        Ok(TcpSocket { manager: self, id })
    }

    pub fn tcp_listen(&mut self, bind_addr: &SocketAddr) -> Result<TcpSocket, ConnectError> {
        let mut socket = {
            let rx_buf = smoltcp::socket::TcpSocketBuffer::new(vec![0; 1024]);
            let tx_buf = smoltcp::socket::TcpSocketBuffer::new(vec![0; 1024]);
            smoltcp::socket::TcpSocket::new(rx_buf, tx_buf)
        };

        let socket = socket.listen(bind_addr.clone()).unwrap();
        let id = SocketId(self.sockets.add(socket));
        self.next_event_delay = None;

        Ok(TcpSocket { manager: self, id })
    }

    /// Injects some data coming from the Ethernet cable.
    ///
    /// Call [`NetworkInterface::next_event`] in order to obtain the result.
    pub fn inject_interface_data(&mut self, data: impl AsRef<[u8]>) {
        let interface_buffers = self.interface_buffers.try_lock().unwrap();
        interface_buffers.device_in_buffer.extend_from_slice(data.as_ref());
        self.next_event_delay = None;
    }

    pub async fn next_event<'a>(&'a mut self) -> NetworkInterfaceEvent<'a> {
        loop {
            if let Some(next_event_delay) = self.next_event_delay.as_mut() {
                next_event_delay.await;
            }
            self.next_event_delay = None;

            let has_anything_changed = match self.ethernet.poll(&mut self.sockets, now().await) {
                Ok(true) => true,
                // TODO: log errors?
                _ => false,
            };

            if has_anything_changed {
                let device_out_buffer = self.device_out_buffer.try_lock().unwrap();
                if !device_out_buffer.is_empty() {
                    let out = mem::replace(&mut *device_out_buffer, Vec::new());
                    return NetworkInterfaceEvent::EthernetCableOut(out);
                }

                for (socket_id, socket_state) in self.socket_state {
                    let smoltcp_socket = self.sockets.get::<smoltcp::socket::TcpSocket>(socket_id).unwrap();

                    // Check if this socket got connected.
                    if !socket_state.is_connected && smoltcp_socket.may_send() {

                    }
                }

                // TODO: TCP socket events
            }

            let next_poll = match self.ethernet.poll_delay(&mut self.sockets, now().await) {
                Some(d) => Into::<Duration>::into(d),
                None => {
                    futures::pending!();
                    continue;
                }
            };

            self.next_event_delay = Some(nametbd_time_interface::Delay::new(next_poll));
        }
    }
}

impl fmt::Debug for NetworkInterface {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("NetworkInterface").finish()
    }
}

impl NetworkInterfaceBuilder {
    /// Adds an IP address and submask that this interface is known to handle.
    // TODO: expand description
    pub fn with_ip_addr(mut self, ip_addr: IpAddr, prefix_len: u8) {
        // TODO: check overlap
        match ip_addr {
            IpAddr::V4(addr) => {
                assert!(prefix_len <= 32);
                self.ip_addresses(From::from(smoltcp::wire::Ipv4Cidr::new(From::from(addr), prefix_len)));
            }
            IpAddr::V6(addr) => {
                assert!(prefix_len <= 64);
                self.ip_addresses(From::from(smoltcp::wire::Ipv6Cidr::new(From::from(addr), prefix_len)));
            }
        }
    }

    /// Builds the [`NetworkInterface`].
    pub fn build(self) -> NetworkInterface {
        // TODO: with_capacity?
        let device_out_buffer = Arc::new(Mutex::new(Vec::new()));
        let device_in_buffer = Arc::new(Mutex::new(Vec::new()));

        let device = RawDevice {
            device_out_buffer: device_out_buffer.clone(),
            device_in_buffer: device_in_buffer.clone(),
        };

        self.ip_addresses.shrink_to_fit();

        let interface = smoltcp::iface::EthernetInterfaceBuilder::new(device)
            .ethernet_addr(smoltcp::wire::EthernetAddress([0x01, 0x00, 0x00, 0x00, 0x00, 0x02])) // TODO:
            .ip_addrs(self.ip_addresses)
            .routes(smoltcp::iface::Routes::new(BTreeMap::new()))
            .neighbor_cache(smoltcp::iface::NeighborCache::new(BTreeMap::new()))
            .finalize();

        NetworkInterface {
            ethernet: interface,
            device_out_buffer,
            device_in_buffer,
        }
    }
}

impl fmt::Debug for NetworkInterfaceBuilder {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("NetworkInterfaceBuilder").finish()
    }
}

impl<'a> TcpSocket<'a> {
    /// Returns the unique identifier of this socket.
    pub fn id(&self) -> SocketId {
        self.id
    }

    /// Instantly drops the socket without a proper shutdown.
    pub fn reset(mut self) {
        self.socket.abort();
    }

    /// Reads the data that has been received on the TCP socket.
    ///
    /// Returns an empty `Vec` if there is no data available.
    pub fn read(&mut self) -> Vec<u8> {
        // TODO:
        Vec::new()
    }

    /// Passes a buffer that the socket will encode into Ethernet frames.
    ///
    /// Only one buffer can be active at any given point in time.
    pub fn set_write_buffer(&mut self, buffer: Vec<u8>) -> Result<(), Vec<u8>> {
        // TODO:
        Err(buffer)
    }

    /// Internal function that returns the `smoltcp::socket::TcpSocket` contained within the set.
    fn socket(&mut self) -> smoltcp::socket::SocketRef<smoltcp::socket::TcpSocket> {
        self.interface.sockets.get::<smoltcp::socket::TcpSocket>(self.id.0)
    }
}

async fn now() -> smoltcp::time::Instant {
    let now = nametbd_time_interface::monotonic_clock().await;
    smoltcp::time::Instant::from_millis((now / 1_000_000) as i64) // TODO: don't use as
}

/// Implementation of `smoltcp::phy::Device`.
struct RawDevice {
    /// Buffer of data to send out to the virtual Ethernet cable.
    /// Shared with the [`NetworkInterface`].
    /// Note: this is a hack around the fact that the API of `smoltcp` wasn't really designed
    /// around being able to retreive the data of the interface.
    device_out_buffer: Arc<Mutex<Vec<u8>>>,

    /// Buffer of data received from the virtual Ethernet cable.
    /// Shared with the [`NetworkInterface`].
    /// Note: this is a hack around the fact that the API of `smoltcp` wasn't really designed
    /// around being able to retreive the data of the interface.
    device_in_buffer: Arc<Mutex<Vec<u8>>>,
}

impl<'a> smoltcp::phy::Device<'a> for RawDevice {
    type RxToken = RawDeviceRxToken<'a>;
    type TxToken = RawDeviceTxToken<'a>;

    fn receive(&'a mut self) -> Option<(Self::RxToken, Self::TxToken)> {
        let buffers = self.buffers.try_lock().unwrap();
        if buffers.device_in_buffer.is_empty() {
            return None;
        }
        
    }

    fn transmit(&'a mut self) -> Option<Self::TxToken> {
        let mut buffer = self.device_out_buffer.try_lock().unwrap();
        None
    }

    fn capabilities(&self) -> phy::DeviceCapabilities {
        let mut caps: phy::DeviceCapabilities = Default::default();
        caps.max_transmission_unit = 9216; // FIXME:
        caps.max_burst_size = None;
        caps.checksum = phy::ChecksumCapabilities::ignored();
        caps
    }
}

struct RawDeviceRxToken<'a> {
    buffer: MutexGuard<'a, Vec<u8>>,
}

impl<'a> phy::RxToken for RawDeviceRxToken<'a> {
    fn consume<R, F>(self, timestamp: Instant, f: F) -> Result<R, smoltcp::Error>
    where
        F: FnOnce(&[u8]) -> Result<R, smoltcp::Error>,
    {
        let result = f(&self.buffer);
        self.buffer.clear();
        result
    }
}

struct RawDeviceTxToken<'a> {
    buffer: MutexGuard<'a, Vec<u8>>,
}

impl<'a> phy::TxToken for RawDeviceTxToken<'a> {
    fn consume<R, F>(self, timestamp: Instant, len: usize, f: F) -> Result<R, smoltcp::Error>
    where
        F: FnOnce(&mut [u8]) -> Result<R, smoltcp::Error>,
    {
        debug_assert!(self.buffer.is_empty());
        // TODO: reserve + set_len instead?
        self.buffer = vec![0; len];
        f(&mut self.buffer)
    }
}
