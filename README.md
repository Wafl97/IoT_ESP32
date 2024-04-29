# IoT Assignment2

Assignment 2 for course in IoT

ESP32 connected to Wi-Fi and subscribed to a command topic on a MQTT broker.
Uses ADC1 on pin GPIO34 to read voltages of a temperature sensor.

## Available commands
### measure
takes 2 arguments:
1. amount: a number of time the device will read and send response 
2. delay: how long in milliseconds the device should wait between responses

> example: ``measure:5,1000``
> 
> This will measure the temperature 5 time with 1 second between reads

## Setup Project

- Install rust (use rustup)
- Install python3, pip and python-venv
- Install espup `cargo install espup`
- Install dependencies `espup install`
- Install ldproxy `cargo install ldproxy`

On Unix-based systems:
  - Run export script `$HOME/export-esp.sh`

## Run Project

`cargo run`

This will build the project, flash  to the ESP32, and monitor the serial port
