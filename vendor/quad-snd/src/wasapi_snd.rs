// https://github.com/floooh/sokol/blob/master/sokol_audio.h
// https://github.com/norse-rs/audir/blob/master/audir/src/wasapi/mod.rs

use crate::PlaySoundParams;

pub use crate::mixer::Playback;

use winapi::shared::guiddef::{CLSID, IID};
use winapi::shared::ksmedia;
use winapi::shared::minwindef::*;
use winapi::shared::mmreg::*;
use winapi::um::audioclient::*;
use winapi::um::audiosessiontypes::*;
use winapi::um::combaseapi::*;
use winapi::um::mmdeviceapi::*;
use winapi::um::objbase::*;
use winapi::um::synchapi::*;
use winapi::um::winbase::*;
use winapi::um::winnt::{HANDLE, LPWSTR};

use std::sync::mpsc;

// thanks sokol_audio!
// https://github.com/floooh/sokol/blob/master/sokol_audio.h#L559
static IID_IAudioClient: IID = IID {
    Data1: 0x1cb9ad4c,
    Data2: 0xdbfa,
    Data3: 0x4c32,
    Data4: [0xb1, 0x78, 0xc2, 0xf5, 0x68, 0xa7, 0x03, 0xb2],
};
static IID_IMMDeviceEnumerator: IID = IID {
    Data1: 0xa95664d2,
    Data2: 0x9614,
    Data3: 0x4f35,
    Data4: [0xa7, 0x46, 0xde, 0x8d, 0xb6, 0x36, 0x17, 0xe6],
};
static CLSID_IMMDeviceEnumerator: CLSID = CLSID {
    Data1: 0xbcde0395,
    Data2: 0xe52f,
    Data3: 0x467c,
    Data4: [0x8e, 0x3d, 0xc4, 0x57, 0x92, 0x91, 0x69, 0x2e],
};
static IID_IAudioRenderClient: IID = IID {
    Data1: 0xf294acfc,
    Data2: 0x3146,
    Data3: 0x4483,
    Data4: [0xa7, 0xbf, 0xad, 0xdc, 0xa7, 0xc2, 0x60, 0xe2],
};
static IID_Devinterface_Audio_Render: IID = IID {
    Data1: 0xe6327cad,
    Data2: 0xdcec,
    Data3: 0x4949,
    Data4: [0xae, 0x8a, 0x99, 0x1e, 0x97, 0x6a, 0x79, 0xd2],
};
static IID_IActivateAudioInterface_Completion_Handler: IID = IID {
    Data1: 0x94ea2b94,
    Data2: 0xe9cc,
    Data3: 0x49e0,
    Data4: [0xc0, 0xff, 0xee, 0x64, 0xca, 0x8f, 0x5b, 0x90],
};

mod consts {
    pub const CHANNELS: u32 = 2;
    pub const SAMPLE_RATE: u32 = 44100;
    pub const BUFFER_FRAMES: u32 = 4096;
}

// Quiz Race patch: the original opened the default device once and waited forever on it, so
// sound stopped for good when the output changed (TV plugged in, headphones, Bluetooth).
// Now the stream is reopened on the new default device; the mixer (and song position) carry on.

struct Stream {
    client: *mut IAudioClient,
    render: *mut IAudioRenderClient,
    frames: u32,
    device_id: Vec<u16>,
}

unsafe fn default_device_id(en: *mut IMMDeviceEnumerator) -> Option<Vec<u16>> {
    let mut dev: *mut IMMDevice = std::ptr::null_mut();
    if (*en).GetDefaultAudioEndpoint(eRender, eConsole, &mut dev) < 0 || dev.is_null() {
        return None;
    }
    let mut id: LPWSTR = std::ptr::null_mut();
    let hr = (*dev).GetId(&mut id);
    (*dev).Release();
    if hr < 0 || id.is_null() {
        return None;
    }
    let mut v = Vec::new();
    let mut q = id;
    while *q != 0 {
        v.push(*q);
        q = q.add(1);
    }
    CoTaskMemFree(id as _);
    Some(v)
}

unsafe fn open_stream(en: *mut IMMDeviceEnumerator, event: HANDLE) -> Option<Stream> {
    let device_id = default_device_id(en)?;
    let mut device: *mut IMMDevice = std::ptr::null_mut();
    if (*en).GetDefaultAudioEndpoint(eRender, eConsole, &mut device) < 0 || device.is_null() {
        return None;
    }
    let mut client: *mut IAudioClient = std::ptr::null_mut();
    let hr = (*device).Activate(&IID_IAudioClient, CLSCTX_ALL, std::ptr::null_mut(), &mut client as *mut _ as _);
    (*device).Release();
    if hr < 0 || client.is_null() {
        return None;
    }

    let format = WAVEFORMATEX {
        nChannels: consts::CHANNELS as _,
        nSamplesPerSec: consts::SAMPLE_RATE as _,
        wFormatTag: WAVE_FORMAT_EXTENSIBLE,
        wBitsPerSample: 32,
        nBlockAlign: consts::CHANNELS as u16 * 4,
        nAvgBytesPerSec: consts::CHANNELS as u32 * consts::SAMPLE_RATE as u32 * 4,
        cbSize: (std::mem::size_of::<WAVEFORMATEXTENSIBLE>() - std::mem::size_of::<WAVEFORMATEX>()) as _,
    };
    const FRONT_LEFT: u32 = 0b0001;
    const FRONT_RIGHT: u32 = 0b0010;
    let format_extensible = WAVEFORMATEXTENSIBLE {
        Format: format,
        Samples: 4 * 8,
        dwChannelMask: FRONT_LEFT | FRONT_RIGHT,
        SubFormat: ksmedia::KSDATAFORMAT_SUBTYPE_IEEE_FLOAT,
    };
    // https://docs.microsoft.com/en-us/windows/win32/coreaudio/audclnt-streamflags-xxx-constants
    const AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM: u32 = 0x80000000;
    const AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY: u32 = 0x08000000;
    let dur = consts::BUFFER_FRAMES as f64 / (consts::SAMPLE_RATE as f64 * 1.0 / 10000000.0);
    let fail = |client: *mut IAudioClient| {
        (*client).Release();
        None
    };
    if (*client).Initialize(
        AUDCLNT_SHAREMODE_SHARED,
        AUDCLNT_STREAMFLAGS_EVENTCALLBACK | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
        dur as _,
        0,
        &format_extensible as *const _ as _,
        std::ptr::null(),
    ) < 0
    {
        return fail(client);
    }
    let mut frames = 0;
    if (*client).GetBufferSize(&mut frames) < 0 {
        return fail(client);
    }
    let mut render: *mut IAudioRenderClient = std::ptr::null_mut();
    if (*client).GetService(&IID_IAudioRenderClient, &mut render as *mut _ as _) < 0 || render.is_null() {
        return fail(client);
    }
    if (*client).SetEventHandle(event) < 0 || (*client).Start() < 0 {
        (*render).Release();
        return fail(client);
    }
    Some(Stream { client, render, frames, device_id })
}

unsafe fn close_stream(s: Stream) {
    (*s.client).Stop();
    (*s.render).Release();
    (*s.client).Release();
}

unsafe fn audio_thread(mut mixer: crate::mixer::Mixer) {
    use std::time::{Duration, Instant};
    CoInitializeEx(std::ptr::null_mut(), COINIT_MULTITHREADED);

    let event = CreateEventA(std::ptr::null_mut(), FALSE, FALSE, std::ptr::null());
    assert!(event.is_null() == false);

    let mut en: *mut IMMDeviceEnumerator = std::ptr::null_mut();
    let hr = CoCreateInstance(
        &CLSID_IMMDeviceEnumerator,
        std::ptr::null_mut(),
        CLSCTX_ALL,
        &IID_IMMDeviceEnumerator,
        &mut en as *mut _ as _,
    );
    assert!(hr >= 0, "CoCreatInstance failed");

    let mut silent = vec![0.0f32; 2205 * consts::CHANNELS as usize];
    loop {
        let Some(stream) = open_stream(en, event) else {
            // no output device right now: keep the mixer going (silently) and try again soon
            mixer.fill_audio_buffer(&mut silent, 2205);
            Sleep(50);
            continue;
        };
        let mut checked = Instant::now();
        loop {
            WaitForSingleObject(event, 100);
            if checked.elapsed() > Duration::from_millis(500) {
                checked = Instant::now();
                if default_device_id(en).as_ref() != Some(&stream.device_id) {
                    break; // output changed (e.g. TV plugged in): reopen on the new default
                }
            }
            let mut padding = 0;
            if (*stream.client).GetCurrentPadding(&mut padding) < 0 {
                break; // device invalidated / unplugged
            }
            let num_frames = stream.frames - padding;
            if num_frames == 0 {
                continue;
            }
            let mut wasapi_buffer: *mut u8 = std::ptr::null_mut();
            if (*stream.render).GetBuffer(num_frames, &mut wasapi_buffer) < 0 || wasapi_buffer.is_null() {
                break;
            }
            let buffer = std::slice::from_raw_parts_mut(wasapi_buffer as *mut f32, num_frames as usize * consts::CHANNELS as usize);
            mixer.fill_audio_buffer(buffer, num_frames as _);
            (*stream.render).ReleaseBuffer(num_frames, 0);
        }
        close_stream(stream);
    }
}

pub struct AudioContext {
    pub(crate) mixer_ctrl: crate::mixer::MixerControl,
}

impl AudioContext {
    pub fn new() -> AudioContext {
        use crate::mixer::Mixer;

        let (mixer_builder, mixer_ctrl) = Mixer::new();
        std::thread::spawn(move || unsafe {
            audio_thread(mixer_builder.build());
        });

        AudioContext { mixer_ctrl }
    }
}

pub struct Sound {
    sound_id: u32,
}

impl Sound {
    pub fn load(ctx: &AudioContext, data: &[u8]) -> Sound {
        let sound_id = ctx.mixer_ctrl.load(data);

        Sound { sound_id }
    }

    pub fn play(&self, ctx: &AudioContext, params: PlaySoundParams) -> Playback {
        ctx.mixer_ctrl.play(self.sound_id, params)
    }

    pub fn stop(&self, ctx: &AudioContext) {
        ctx.mixer_ctrl.stop_all(self.sound_id);
    }

    pub fn set_volume(&self, ctx: &AudioContext, volume: f32) {
        ctx.mixer_ctrl.set_volume_all(self.sound_id, volume);
    }

    pub fn delete(&self, ctx: &AudioContext) {
        ctx.mixer_ctrl.delete(self.sound_id);
    }
}
