import requests
from typing import Optional
import time


class PhyController:

    def __init__(
            self, url,
            center_freq: Optional[int] = None, rx_freq_offset: Optional[tuple[int, int]] = None,
            tx_freq_offset: Optional[tuple[int, int]] = None,
            rx_freq: Optional[tuple[int, int]] = None, tx_freq: Optional[tuple[int, int]] = None,
            rx_gain: tuple[int, int] = (60, 60), tx_gain: tuple[int, int] = (40, 40),
            sample_rate: tuple[int, int] = (4e6, 4e6),
            rx_device_channel: int = 0,
            tx_device_channel: int = 0,
    ):
        self.url = url
        request = requests.get(url)
        request_json = request.json()
        self.center_offset_mode = center_freq is not None
        self.center_freq = center_freq
        self.rx_freq_offset, self.tx_freq_offset, self.rx_freq, self.tx_freq = (None, ) * 4
        if self.center_offset_mode:
            assert rx_freq_offset is not None and tx_freq_offset is not None
            self.rx_freq_offset = list(rx_freq_offset)
            self.tx_freq_offset = list(tx_freq_offset)
        else:
            assert rx_freq is not None and tx_freq is not None
            self.rx_freq = list(rx_freq)
            self.tx_freq = list(tx_freq)
        self.blocks = request_json["blocks"]
        self.current_phy = 0

        self.rx_gain = list(rx_gain)
        self.tx_gain = list(tx_gain)
        self.sample_rate = list(sample_rate)

        self.rx_device_channel = rx_device_channel
        self.tx_device_channel = tx_device_channel

        # get id of relevant blocks
        source_selector_id = -1
        sink_selector_id = -1
        message_selector_id = -1
        soapy_source_id = -1
        soapy_sink_id = -1
        lora_frame_sync_id = -1
        lora_modulate_id = -1

        for block in self.blocks:
            # print(block["instance_name"])
            if block["instance_name"] == "Selector<2, 1>_0":
                source_selector_id = block["id"]
            if block["instance_name"] == "Selector<1, 2>_0":
                sink_selector_id = block["id"]
            if block["instance_name"] == "MessageSelector_0":
                message_selector_id = block["id"]
            if block["instance_name"] == "Sink_0":
                soapy_sink_id = block["id"]
            if block["instance_name"] == "Source_0":
                soapy_source_id = block["id"]
            if "FrameSync" in block["instance_name"]:
                lora_frame_sync_id = block["id"]
            if "Modulate" in block["instance_name"]:
                lora_modulate_id = block["id"]

        if (source_selector_id == -1) or (sink_selector_id == -1) or (soapy_source_id == -1) or (
                soapy_sink_id == -1) or (message_selector_id == -1):
            if source_selector_id == -1:
                print("Cannot find source selector!")
            if sink_selector_id == -1:
                print("Cannot find sink selector!")
            if soapy_source_id == -1:
                print("Cannot find soapy source")
            if soapy_sink_id == -1:
                print("Cannot find soapy sink")
            if message_selector_id == -1:
                print("Cannot find message selector")

        self.source_selector_url = "{0}block/{1}/call/0/".format(url, source_selector_id)
        self.sink_selector_url = "{0}block/{1}/call/1/".format(url, sink_selector_id)
        self.message_selector_url = "{0}block/{1}/call/1/".format(url, message_selector_id)
        self.soapy_source_freq_url = "{0}block/{1}/call/0/".format(url, soapy_source_id)
        self.soapy_source_gain_url = "{0}block/{1}/call/1/".format(url, soapy_source_id)
        self.soapy_source_sample_rate_url = "{0}block/{1}/call/2/".format(url, soapy_source_id)
        self.soapy_source_freq_offset_url = "{0}block/{1}/call/4/".format(url, soapy_source_id)
        self.soapy_sink_freq_url = "{0}block/{1}/call/0/".format(url, soapy_sink_id)
        self.soapy_sink_gain_url = "{0}block/{1}/call/1/".format(url, soapy_sink_id)
        self.soapy_sink_sample_rate_url = "{0}block/{1}/call/2/".format(url, soapy_sink_id)
        self.soapy_sink_freq_offset_url = "{0}block/{1}/call/4/".format(url, soapy_sink_id)


        self.lora_frame_sync_center_freq_url = "{0}block/{1}/call/1/".format(url, lora_frame_sync_id)
        self.lora_frame_sync_bw_url = "{0}block/{1}/call/0/".format(url, lora_frame_sync_id)
        self.lora_modulate_bw_url = "{0}block/{1}/call/0/".format(url, lora_modulate_id)
        self.lora_modulate_sr_url = "{0}block/{1}/call/1/".format(url, lora_modulate_id)

    def use_center_frequency_offset_mode(self, use: bool):
        """
        determines if center frequency and offset frequency is used to tune the sdr or a single frequency
        """
        self.center_offset_mode = use

    # def set_rx_frequency_config(self, phy, frequency):
    #     """
    #     sets rx frequency  of the corresponding phy. applies config on next phy selection
    #     """
    #     self.rx_freq[phy] = frequency
    #
    # def set_tx_frequency_config(self, phy, frequency):
    #     """
    #     sets tx frequency  of the corresponding phy. applies config on next phy selection
    #     """
    #     self.tx_freq[phy] = frequency

    def set_rx_gain_config(self, phy, gain):
        """
        set rx gain  of the corresponding phy. applies config on next phy selection
        """
        self.rx_gain[phy] = gain

    def set_tx_gain_config(self, phy, gain):
        """
        set tx gain  of the corresponding phy. applies config on next phy selection
        """
        self.tx_gain[phy] = gain

    def set_sample_rate_config(self, phy, sample_rate):
        """
        sets sample rate of the corresponding phy. applies config on next phy selection
        """
        self.sample_rate[phy] = sample_rate

    # def _set_sample_channel_config(self, receiver, transmitter):
    #     """
    #     sets channel of receiver/transmitter. applies config on next phy selection
    #     channel needs to match the physical configuration/connection.
    #     can not be set dynamically
    #     """
    #     self.rx_channel = receiver
    #     self.tx_channel = transmitter

    def set_center_frequency_config(self, freq):
        """
        set center frquency of the corresponding phy. applies config on next phy selection
        """
        self.center_freq = freq

    def set_rx_frequency_offset_config(self, phy, offset):
        """
        set frequency offset of the corresponding phy. applies config on next phy selection
        """
        self.rx_freq_offset[phy] = offset

    def set_tx_frequency_offset_config(self, phy, offset):
        """
        set frequency offset of the corresponding phy. applies config on next phy selection
        """
        self.tx_freq_offset[phy] = offset

    """
    methods for direct manipulation (these will not update the stored settings, but only apply the change temporally)
    """

    def set_rx_frequency(self, frequency):
        """
        sets rx frequency via message handler
        """
        requests.post(self.soapy_source_freq_url, json={"F64": int(frequency)})

    def set_tx_frequency(self, frequency):
        """
        sets tx frequency via message handler
        """
        requests.post(self.soapy_sink_freq_url, json={"F64": int(frequency)})

    def set_rx_gain(self, frequency):
        """
        set rx gain via message handler
        """
        requests.post(self.soapy_source_gain_url, json={"F64": int(frequency)})

    def set_tx_gain(self, gain):
        """
        set tx gain via message handler
        """
        requests.post(self.soapy_sink_gain_url, json={"F64": int(gain)})

    def set_rx_sample_rate(self, sample_rate):
        """
        sets sample rate via message handler
        """
        requests.post(self.soapy_source_sample_rate_url, json={"F64": int(sample_rate)})

    def set_tx_sample_rate(self, sample_rate):
        """
        sets sample rate via message handler
        """
        requests.post(self.soapy_sink_sample_rate_url, json={"F64": int(sample_rate)})

    # def set_rx_center_frequency(self, freq, channel):
    #     """
    #     set center frquency via message handler
    #     """
    #     requests.post(self.soapy_source_center_freq_url, json={"VecPmt": [{"F64": int(freq)}, {"U32": int(channel)}]})

    # def set_tx_center_frequency(self, freq, channel):
    #     """
    #     set center frquency via message handler
    #     """
    #     requests.post(self.soapy_sink_center_freq_url, json={"VecPmt": [{"F64": int(freq)}, {"U32": int(channel)}]})

    def set_rx_frequency_offset(self, offset, channel):
        """
        set frequency offset via message handler
        """
        requests.post(self.soapy_source_freq_offset_url, json={"VecPmt": [{"F64": int(offset)}, {"U32": int(channel)}]})

    def set_tx_frequency_offset(self, offset, channel):
        """
        set frequency offset via message handler
        """
        requests.post(self.soapy_sink_center_freq_url, json={"VecPmt": [{"F64": int(offset)}, {"U32": int(channel)}]})

    """
    switch the phy layer
    """

    def select_phy(self, phy):
        """
        select the PHY protocol (WLAN = 0, Bluetooth =1)
        """
        requests.post(self.soapy_source_freq_url, json={"F64": int(self.center_freq)})
        requests.post(self.soapy_sink_freq_url, json={"F64": int(self.center_freq)})
        requests.post(
            self.soapy_source_freq_offset_url,
            # json={"VecPmt": [{"F64": int(self.rx_freq_offset[phy])}, {"U32": self.rx_device_channel}]}
            json={"F64": int(self.rx_freq_offset[phy])}
        )
        requests.post(
            self.soapy_sink_freq_offset_url,
            # json={"VecPmt": [{"F64": int(self.tx_freq_offset[phy])}, {"U32": self.tx_device_channel}]}
            json={"F64": int(self.tx_freq_offset[phy])}
        )
        requests.post(self.source_selector_url, json={"U32": phy})
        requests.post(self.sink_selector_url, json={"U32": phy})
        requests.post(self.message_selector_url, json={"U32": phy})
        requests.post(self.soapy_source_gain_url, json={"F64": int(self.rx_gain[phy])})
        requests.post(self.soapy_source_sample_rate_url, json={"F64": int(self.sample_rate[phy])})
        requests.post(self.soapy_sink_gain_url, json={"F64": int(self.tx_gain[phy])})
        requests.post(self.soapy_sink_sample_rate_url, json={"F64": int(self.sample_rate[phy])})
#         if phy == 1:
# # #             requests.post(self.lora_frame_sync_center_freq_url, json={"Usize": int(self.center_freq) - int(self.rx_freq_offset[phy])})
#             requests.post(self.lora_frame_sync_bw_url, json={"Usize": int(self.sample_rate[phy])})
#             requests.post(self.lora_modulate_bw_url, json={"Usize": int(self.sample_rate[phy])})
#             requests.post(self.lora_modulate_sr_url, json={"Usize": int(self.sample_rate[phy])})
        if self.center_offset_mode:
#             requests.post(
#                 self.soapy_source_freq_url,
#                 # json={"VecPmt": [{"F64": int(self.center_freq)}, {"U32": self.rx_device_channel}]}  # TODO
#                 json={"F64": int(self.center_freq)}
#             )
#             requests.post(
#                 self.soapy_sink_freq_url,
#                 # json={"VecPmt": [{"F64": int(self.center_freq)}, {"U32": self.tx_device_channel}]}
#                 json={"F64": int(self.center_freq)}
#             )
            pass
        else:
            raise NotImplementedError()
            requests.post(self.soapy_source_freq_url, json={"F64": int(self.rx_freq[phy])})
            requests.post(self.soapy_sink_freq_url, json={"F64": int(self.tx_freq[phy])})
        self.current_phy = phy

    def switch_phy(self):
        """
        switches to the other phy
        """
        if self.current_phy == 0:
            new_phy = 1
        else:
            new_phy = 0
        self.select_phy(new_phy)
