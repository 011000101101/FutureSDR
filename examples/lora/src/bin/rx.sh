#!/bin/bash

BUILD_TYPE=""
#BUILD_TYPE="--release"
CENTER_FREQ="--frequency=868100000"
DEVICE_FILTER="--device-filter=driver=aaronia_http,tx_url=http://172.18.0.1:54665,url=http://172.18.0.1:54664"
RX_GAIN="--gain 10"
RX_ANTENNA=""
SPREADING_FACTOR="--spreading-factor 12"
BANDWIDTH="--bandwidth 125000"

export FUTURESDR_LOG_LEVEL=debug
export RUST_BACKTRACE=full

cargo run --bin rx ${BUILD_TYPE} -- ${SPREADING_FACTOR} ${BANDWIDTH} ${CENTER_FREQ} ${DEVICE_FILTER} ${RX_GAIN} ${RX_ANTENNA} ${SAMPLE_RATE}
